//! Authenticated HTTP resource endpoints served on the private Unix control socket.
//!
//! The remote connector authenticates callers (origin + bearer token), strips caller
//! credentials, and forwards resource reads here over the same-user Unix socket.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use axum::Router;
use axum::body::Bytes;
use axum::extract::Path as AxumPath;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::http::header::CACHE_CONTROL;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use axum::routing::post;
use base64::Engine;
use codex_app_server_protocol::DynamicToolCallOutputContentItem;
use codex_app_server_protocol::GeneratedImageRef;
use codex_app_server_protocol::ImageReference;
use codex_app_server_protocol::ThreadItemPayload;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::UserInput;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use tokio::io::AsyncReadExt;

use crate::request_processors::ThreadRequestProcessor;

const PROTOCOL_HEADER: &str = "Mekugi-Protocol";
const PROTOCOL_VERSION: &str = "9";
const UPLOAD_LIMIT: usize = 32 << 20;
const DEFAULT_RESOURCE_BODY_LIMIT: usize = 1024;
const ATTACHMENT_IMAGE_BODY_LIMIT: usize = 64 << 10;
const ENVELOPE_OVERHEAD: usize = 64 << 10;

#[derive(Clone)]
struct ResourceState {
    thread_processor: Arc<OnceLock<Arc<ThreadRequestProcessor>>>,
}

#[derive(Serialize)]
struct ResourceErrorData {
    status: u16,
    outcome: &'static str,
}

#[derive(Serialize)]
struct ResourceError {
    code: i64,
    message: String,
    data: ResourceErrorData,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceParams {
    #[serde(default)]
    thread: String,
    #[serde(default)]
    startup: String,
    #[serde(default)]
    subscription: String,
    #[serde(default)]
    body: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentBody {
    #[serde(default)]
    thread: String,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    item_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageBody {
    #[serde(default)]
    item_id: Option<String>,
    #[serde(default)]
    index: Option<usize>,
    #[serde(default)]
    image: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttachmentImageBody {
    #[serde(default)]
    path: String,
    #[serde(default)]
    workspace: String,
}

pub(crate) fn router(thread_processor: Arc<OnceLock<Arc<ThreadRequestProcessor>>>) -> Router {
    Router::new()
        .route("/api/version", get(version_handler))
        .route("/api/resources/{kind}", post(resource_handler))
        .with_state(ResourceState { thread_processor })
}

fn base_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        axum::http::header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        axum::http::header::HeaderName::from_static("mekugi-protocol"),
        HeaderValue::from_static(PROTOCOL_VERSION),
    );
    headers
}

fn error_response(status: StatusCode, message: impl Into<String>) -> Response {
    let payload = ResourceError {
        code: -32000,
        message: message.into(),
        data: ResourceErrorData {
            status: status.as_u16(),
            outcome: "not_admitted",
        },
    };
    let body = serde_json::to_vec(&payload).unwrap_or_default();
    let mut headers = base_headers();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    (status, headers, body).into_response()
}

fn json_response(value: &impl Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut headers = base_headers();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            (StatusCode::OK, headers, body).into_response()
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Resource content unavailable.",
        ),
    }
}

fn binary_response(mime: &'static str, data: Vec<u8>) -> Response {
    let mut headers = base_headers();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(mime));
    (StatusCode::OK, headers, data).into_response()
}

fn check_protocol_header(headers: &HeaderMap) -> Result<(), Response> {
    let mut values = headers.get_all(PROTOCOL_HEADER).iter();
    let Some(first) = values.next() else {
        return Err(error_response(
            StatusCode::CONFLICT,
            "incompatible Mekugi control protocol",
        ));
    };
    if values.next().is_some() || first.as_bytes() != PROTOCOL_VERSION.as_bytes() {
        return Err(error_response(
            StatusCode::CONFLICT,
            "incompatible Mekugi control protocol",
        ));
    }
    Ok(())
}

#[tracing::instrument(skip_all)]
async fn version_handler(headers: HeaderMap) -> Response {
    if let Err(response) = check_protocol_header(&headers) {
        return response;
    }
    json_response(&serde_json::json!({
        "ready": true,
        "version": PROTOCOL_VERSION,
    }))
}

#[tracing::instrument(skip_all, fields(kind = %kind))]
async fn resource_handler(
    AxumPath(kind): AxumPath<String>,
    State(state): State<ResourceState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = check_protocol_header(&headers) {
        return response;
    }
    let limit = match kind.as_str() {
        "attachment-image" => ATTACHMENT_IMAGE_BODY_LIMIT,
        "image" | "content" => DEFAULT_RESOURCE_BODY_LIMIT,
        _ => return error_response(StatusCode::NOT_FOUND, "Unknown resource endpoint."),
    };
    if body.len() > limit + ENVELOPE_OVERHEAD {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Resource request body too large.",
        );
    }
    let params: ResourceParams = match serde_json::from_slice(&body) {
        Ok(params) => params,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid resource request."),
    };
    if !params.subscription.is_empty() || !params.startup.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "Invalid resource request.");
    }
    if let Some(ref inner) = params.body {
        let serialized_len = serde_json::to_vec(inner).map(|v| v.len()).unwrap_or(0);
        if serialized_len > limit {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Resource request body too large.",
            );
        }
    }
    let Some(thread_processor) = state.thread_processor.get() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "App server is still starting.",
        );
    };
    match kind.as_str() {
        "content" => handle_content(thread_processor, params).await,
        "image" => handle_image(thread_processor, params).await,
        "attachment-image" => handle_attachment_image(thread_processor, params).await,
        _ => error_response(StatusCode::NOT_FOUND, "Unknown resource endpoint."),
    }
}

async fn handle_content(
    thread_processor: &ThreadRequestProcessor,
    params: ResourceParams,
) -> Response {
    let body_value = params.body.unwrap_or_else(|| serde_json::json!({}));
    let content_body: ContentBody = match serde_json::from_value(body_value) {
        Ok(parsed) => parsed,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid content request."),
    };
    let thread_id = if !params.thread.is_empty() {
        params.thread
    } else if !content_body.thread.is_empty() {
        content_body.thread
    } else {
        return error_response(StatusCode::BAD_REQUEST, "A thread id is required.");
    };
    let response = match thread_processor
        .thread_read_response_inner(ThreadReadParams {
            thread_id,
            include_turns: true,
        })
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "The requested thread content is unavailable.",
            );
        }
    };
    if let Some(ref item_id) = content_body.item_id {
        for turn in &response.thread.turns {
            if let Some(ref turn_id) = content_body.turn_id
                && turn.id != *turn_id
            {
                continue;
            }
            if let Some(item) = turn.items.iter().find(|item| item.id() == item_id) {
                return json_response(&serde_json::json!({
                    "threadId": response.thread.id,
                    "turnId": turn.id,
                    "item": item,
                }));
            }
        }
        return error_response(
            StatusCode::NOT_FOUND,
            "The requested detail item is unavailable.",
        );
    }
    if let Some(ref turn_id) = content_body.turn_id {
        if let Some(turn) = response
            .thread
            .turns
            .iter()
            .find(|turn| turn.id == *turn_id)
        {
            return json_response(&serde_json::json!({
                "threadId": response.thread.id,
                "turn": turn,
            }));
        }
        return error_response(
            StatusCode::NOT_FOUND,
            "The requested turn content is unavailable.",
        );
    }
    json_response(&serde_json::json!({
        "thread": response.thread,
    }))
}

fn image_identity(source: &str) -> String {
    let digest = Sha256::digest(source.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

async fn handle_image(
    thread_processor: &ThreadRequestProcessor,
    params: ResourceParams,
) -> Response {
    let Some(body_value) = params.body else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid image request.");
    };
    let image_body: ImageBody = match serde_json::from_value(body_value) {
        Ok(parsed) => parsed,
        Err(_) => return error_response(StatusCode::BAD_REQUEST, "Invalid image request."),
    };
    if params.thread.is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "A thread id is required.");
    }
    let response = match thread_processor
        .thread_read_response_inner(ThreadReadParams {
            thread_id: params.thread,
            include_turns: true,
        })
        .await
    {
        Ok(response) => response,
        Err(_) => {
            return error_response(
                StatusCode::NOT_FOUND,
                "This attached image is no longer available.",
            );
        }
    };
    let workspace = PathBuf::from(response.thread.cwd.as_str());
    let matched_source = find_published_image_source(&response.thread.turns, &image_body);
    let Some(source) = matched_source else {
        return error_response(
            StatusCode::NOT_FOUND,
            "This attached image is no longer available.",
        );
    };
    read_image_source(&workspace, &source, ImageReadMode::Published).await
}

fn extract_item_image_sources(payload: &ThreadItemPayload) -> Vec<String> {
    let mut sources = Vec::new();
    match payload {
        ThreadItemPayload::UserMessage { content, .. } => {
            for input in content {
                match input {
                    UserInput::LocalImage { path, .. } => {
                        sources.push(path.to_string_lossy().into_owned());
                    }
                    UserInput::Image {
                        image: ImageReference::Inline { url },
                        ..
                    } => {
                        sources.push(url.clone());
                    }
                    _ => {}
                }
            }
        }
        ThreadItemPayload::ImageView { path, .. } => {
            sources.push(path.as_str().to_string());
        }
        ThreadItemPayload::ImageGeneration(item) => {
            if let Some(saved_path) = &item.saved_path {
                sources.push(saved_path.to_string_lossy().into_owned());
            }
            if let GeneratedImageRef::Inline { result } = &item.image
                && !result.is_empty()
            {
                if result.starts_with("data:") {
                    sources.push(result.clone());
                } else {
                    sources.push(format!("data:image/png;base64,{result}"));
                }
            }
        }
        ThreadItemPayload::DynamicToolCall {
            content_items: Some(items),
            ..
        } => {
            for item in items {
                if let DynamicToolCallOutputContentItem::InputImage { image_url } = item {
                    sources.push(image_url.clone());
                }
            }
        }
        ThreadItemPayload::McpToolCall {
            result: Some(result),
            ..
        } => {
            for block in &result.content {
                let Some(obj) = block.as_object() else {
                    continue;
                };
                if obj.get("type").and_then(serde_json::Value::as_str) != Some("image") {
                    continue;
                }
                if let Some(data) = obj.get("data").and_then(serde_json::Value::as_str) {
                    let mime = obj
                        .get("mimeType")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("image/png");
                    sources.push(format!("data:{mime};base64,{data}"));
                } else if let Some(url) = obj
                    .get("url")
                    .or_else(|| obj.get("imageUrl"))
                    .and_then(serde_json::Value::as_str)
                {
                    sources.push(url.to_string());
                }
            }
        }
        _ => {}
    }
    sources
}

fn find_published_image_source(
    turns: &[codex_app_server_protocol::Turn],
    image_body: &ImageBody,
) -> Option<String> {
    for turn in turns {
        for item in &turn.items {
            if let Some(ref expected_item_id) = image_body.item_id
                && item.id() != expected_item_id
            {
                continue;
            }
            for (image_index, candidate) in
                extract_item_image_sources(&item.payload).into_iter().enumerate()
            {
                let index_ok = image_body.index.is_none_or(|idx| idx == image_index);
                let identity_ok =
                    image_body.image.is_empty() || image_identity(&candidate) == image_body.image;
                if index_ok && identity_ok {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

async fn handle_attachment_image(
    thread_processor: &ThreadRequestProcessor,
    params: ResourceParams,
) -> Response {
    let Some(body_value) = params.body else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid attachment image request.");
    };
    let request: AttachmentImageBody = match serde_json::from_value(body_value) {
        Ok(parsed) => parsed,
        Err(_) => {
            return error_response(StatusCode::BAD_REQUEST, "Invalid attachment image request.");
        }
    };
    if request.path.is_empty()
        || request.path.starts_with("data:")
        || request.path.contains("://")
    {
        return error_response(
            StatusCode::BAD_REQUEST,
            "Choose an attachment path on the execution machine.",
        );
    }
    let workspace = if !params.thread.is_empty() {
        if !request.workspace.is_empty() {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Attachment previews require the live thread's workspace.",
            );
        }
        let response = match thread_processor
            .thread_read_response_inner(ThreadReadParams {
                thread_id: params.thread,
                include_turns: false,
            })
            .await
        {
            Ok(response) => response,
            Err(_) => {
                return error_response(
                    StatusCode::NOT_FOUND,
                    "The requested thread is unavailable.",
                );
            }
        };
        PathBuf::from(response.thread.cwd.as_str())
    } else {
        let path = PathBuf::from(&request.workspace);
        if !path.is_absolute() {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Choose an existing absolute workspace directory.",
            );
        }
        let offered_root = thread_processor.config.cwd.as_path();
        let (Ok(canonical_offered), Ok(canonical_req)) = (
            tokio::fs::canonicalize(offered_root).await,
            tokio::fs::canonicalize(&path).await,
        ) else {
            return unavailable_image_response();
        };
        if !canonical_req.starts_with(&canonical_offered) {
            return unavailable_image_response();
        }
        path
    };
    read_image_source(&workspace, &request.path, ImageReadMode::WorkspaceConfined).await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImageReadMode {
    Published,
    WorkspaceConfined,
}

fn unavailable_image_response() -> Response {
    error_response(
        StatusCode::NOT_FOUND,
        "The attached image file is unavailable.",
    )
}

#[cfg(unix)]
fn open_confined_file_sync(canonical_workspace: &Path, relative: &Path) -> std::io::Result<std::fs::File> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;
    use std::os::fd::OwnedFd;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Component;

    let root_c = CString::new(canonical_workspace.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid workspace"))?;
    let root_fd = unsafe {
        libc::open(
            root_c.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if root_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut dir_fd = unsafe { OwnedFd::from_raw_fd(root_fd) };
    let mut components = Vec::new();
    for comp in relative.components() {
        match comp {
            Component::Normal(part) => components.push(part),
            Component::CurDir => {}
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "path escapes workspace",
                ));
            }
        }
    }
    let Some((file_name, parents)) = components.split_last() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty path"));
    };
    for part in parents {
        let part_c = CString::new(part.as_bytes())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid segment"))?;
        let next_fd = unsafe {
            libc::openat(
                std::os::fd::AsRawFd::as_raw_fd(&dir_fd),
                part_c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if next_fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        dir_fd = unsafe { OwnedFd::from_raw_fd(next_fd) };
    }
    let file_c = CString::new(file_name.as_bytes())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid filename"))?;
    let file_fd = unsafe {
        libc::openat(
            std::os::fd::AsRawFd::as_raw_fd(&dir_fd),
            file_c.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if file_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let file = unsafe { std::fs::File::from_raw_fd(file_fd) };
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

#[tracing::instrument(level = "debug", skip_all)]
async fn open_image_file(
    canonical_workspace: Option<&Path>,
    resolved: &Path,
    mode: ImageReadMode,
) -> std::io::Result<tokio::fs::File> {
    if mode == ImageReadMode::WorkspaceConfined {
        #[cfg(unix)]
        {
            let ws = canonical_workspace
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "missing workspace"))?
                .to_path_buf();
            let rel = resolved
                .strip_prefix(&ws)
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::PermissionDenied, "outside workspace"))?
                .to_path_buf();
            let std_file = tokio::task::spawn_blocking(move || open_confined_file_sync(&ws, &rel))
                .await
                .map_err(|err| std::io::Error::other(format!("open confined image task failed: {err}")))??;
            return Ok(tokio::fs::File::from_std(std_file));
        }
    }
    let _ = canonical_workspace;
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    options.open(resolved).await
}

async fn read_image_source(workspace: &Path, source: &str, mode: ImageReadMode) -> Response {
    let data = if let Some(rest) = source.strip_prefix("data:") {
        if mode == ImageReadMode::WorkspaceConfined {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Choose an attachment path on the execution machine.",
            );
        }
        let Some((header, encoded)) = rest.split_once(',') else {
            return error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Unsupported attached image data.",
            );
        };
        let max_encoded = UPLOAD_LIMIT.div_ceil(3) * 4;
        if !header.ends_with(";base64") || encoded.len() > max_encoded {
            return error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Unsupported attached image data.",
            );
        }
        match base64::engine::general_purpose::STANDARD.decode(encoded) {
            Ok(bytes) => bytes,
            Err(_) => {
                return error_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "The attached image could not be read within the upload limit.",
                );
            }
        }
    } else {
        if source.contains("://") {
            return error_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "Remote image URLs are not loaded automatically.",
            );
        }
        let candidate = Path::new(source);
        let (canonical_ws, resolved) = if mode == ImageReadMode::WorkspaceConfined {
            let Ok(canonical_workspace) = tokio::fs::canonicalize(workspace).await else {
                return unavailable_image_response();
            };
            if !tokio::fs::metadata(&canonical_workspace)
                .await
                .is_ok_and(|m| m.is_dir())
            {
                return unavailable_image_response();
            }
            let joined = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                canonical_workspace.join(candidate)
            };
            let Ok(canonical_resolved) = tokio::fs::canonicalize(&joined).await else {
                return unavailable_image_response();
            };
            if !canonical_resolved.starts_with(&canonical_workspace) {
                return unavailable_image_response();
            }
            (Some(canonical_workspace), canonical_resolved)
        } else if candidate.is_absolute() {
            (None, candidate.to_path_buf())
        } else {
            if !workspace.is_absolute() {
                return error_response(StatusCode::BAD_REQUEST, "The image has no workspace.");
            }
            (None, workspace.join(candidate))
        };
        let mut file = match open_image_file(canonical_ws.as_deref(), &resolved, mode).await {
            Ok(file) => file,
            Err(_) => return unavailable_image_response(),
        };
        if mode == ImageReadMode::WorkspaceConfined && !file.metadata().await.is_ok_and(|m| m.is_file()) {
            return unavailable_image_response();
        }
        let mut bytes = Vec::new();
        let mut limited = (&mut file).take((UPLOAD_LIMIT + 1) as u64);
        if limited.read_to_end(&mut bytes).await.is_err() {
            return error_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "The attached image could not be read within the upload limit.",
            );
        }
        bytes
    };
    if data.len() > UPLOAD_LIMIT {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "The attached image could not be read within the upload limit.",
        );
    }
    let Some(mime) = detect_raster_mime(&data) else {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "The attachment is not a supported raster image.",
        );
    };
    binary_response(mime, data)
}

fn detect_raster_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn detects_supported_raster_images_only() {
        assert_eq!(
            detect_raster_mime(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some("image/png")
        );
        assert_eq!(detect_raster_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(detect_raster_mime(b"GIF89a..."), Some("image/gif"));
        assert_eq!(detect_raster_mime(b"RIFF0000WEBP"), Some("image/webp"));
        assert_eq!(detect_raster_mime(b"<svg></svg>"), None);
        assert_eq!(detect_raster_mime(b"plain text"), None);
    }

    #[test]
    fn finds_published_user_and_generated_and_tool_images() {
        use codex_app_server_protocol::ImageGenerationItem;
        use codex_app_server_protocol::McpToolCallResult;
        use codex_app_server_protocol::McpToolCallStatus;
        use codex_app_server_protocol::ThreadItem;
        use codex_app_server_protocol::Turn;
        use codex_app_server_protocol::TurnStatus;

        let user_url = "data:image/png;base64,iVBORw0KGgo=".to_string();
        let gen_b64 = "iVBORw0KGgoAAAANSUhEUg==".to_string();
        let gen_url = format!("data:image/png;base64,{gen_b64}");
        let mcp_url = "data:image/jpeg;base64,/9j/4AAQ".to_string();

        let turn = Turn {
            id: "turn-1".to_string(),
            root_turn_id: None,
            items: vec![
                ThreadItem::untimed(ThreadItemPayload::UserMessage {
                    id: "user-1".to_string(),
                    client_id: None,
                    content: vec![UserInput::Image {
                        image: ImageReference::Inline {
                            url: user_url.clone(),
                        },
                        detail: None,
                    }],
                }),
                ThreadItem::untimed(ThreadItemPayload::ImageGeneration(ImageGenerationItem {
                    id: "gen-1".to_string(),
                    status: "completed".to_string(),
                    revised_prompt: None,
                    image: GeneratedImageRef::Inline { result: gen_b64 },
                    transparent_background: None,
                    failure: None,
                    saved_path: None,
                    input_images: Vec::new(),
                    imagegen_request_id: None,
                    generation_id: None,
                })),
                ThreadItem::untimed(ThreadItemPayload::McpToolCall {
                    id: "mcp-1".to_string(),
                    server: "srv".to_string(),
                    environment_id: None,
                    tool: "render".to_string(),
                    status: McpToolCallStatus::Completed,
                    arguments: serde_json::json!({}),
                    app_context: None,
                    mcp_app_resource_uri: None,
                    mcp_app_ui: None,
                    plugin_id: None,
                    read_only_hint: None,
                    result: Some(Box::new(McpToolCallResult {
                        content: vec![serde_json::json!({
                            "type": "image",
                            "mimeType": "image/jpeg",
                            "data": "/9j/4AAQ"
                        })],
                        structured_content: None,
                        meta: None,
                    })),
                    error: None,
                    duration_ms: None,
                }),
            ],
            items_view: Default::default(),
            status: TurnStatus::Completed,
            error: None,
            started_at: None,
            completed_at: None,
            duration_ms: None,
        };

        assert_eq!(
            find_published_image_source(
                std::slice::from_ref(&turn),
                &ImageBody {
                    item_id: Some("user-1".to_string()),
                    index: Some(0),
                    image: image_identity(&user_url),
                }
            ),
            Some(user_url)
        );
        assert_eq!(
            find_published_image_source(
                std::slice::from_ref(&turn),
                &ImageBody {
                    item_id: Some("gen-1".to_string()),
                    index: Some(0),
                    image: image_identity(&gen_url),
                }
            ),
            Some(gen_url)
        );
        assert_eq!(
            find_published_image_source(
                std::slice::from_ref(&turn),
                &ImageBody {
                    item_id: Some("mcp-1".to_string()),
                    index: Some(0),
                    image: image_identity(&mcp_url),
                }
            ),
            Some(mcp_url)
        );
    }

    #[tokio::test]
    async fn confined_attachment_image_reads_only_workspace_files_and_hides_outside_existence() {
        let root = tempfile::tempdir().expect("tempdir");
        let workspace = root.path().join("workspace");
        let outside = root.path().join("outside");
        tokio::fs::create_dir_all(&workspace).await.expect("mkdir ws");
        tokio::fs::create_dir_all(&outside).await.expect("mkdir out");

        let png_bytes = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
        let inside_png = workspace.join("pic.png");
        let inside_txt = workspace.join("notes.txt");
        let outside_png = outside.join("secret.png");
        let outside_txt = outside.join("secret.txt");
        tokio::fs::write(&inside_png, png_bytes).await.expect("write inside png");
        tokio::fs::write(&inside_txt, b"plain text").await.expect("write inside txt");
        tokio::fs::write(&outside_png, png_bytes).await.expect("write outside png");
        tokio::fs::write(&outside_txt, b"not an image").await.expect("write outside txt");

        let ok_rel = read_image_source(&workspace, "pic.png", ImageReadMode::WorkspaceConfined).await;
        assert_eq!(ok_rel.status(), StatusCode::OK);
        assert_eq!(
            ok_rel.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()),
            Some("image/png")
        );

        let ok_abs = read_image_source(&workspace, &inside_png.to_string_lossy(), ImageReadMode::WorkspaceConfined).await;
        assert_eq!(ok_abs.status(), StatusCode::OK);

        let inside_non_image = read_image_source(&workspace, "notes.txt", ImageReadMode::WorkspaceConfined).await;
        assert_eq!(inside_non_image.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let sub_dir = workspace.join("subdir");
        tokio::fs::create_dir_all(&sub_dir).await.expect("mkdir subdir");
        let dir_resp = read_image_source(&workspace, "subdir", ImageReadMode::WorkspaceConfined).await;
        assert_eq!(dir_resp.status(), StatusCode::NOT_FOUND);

        let trav_png = read_image_source(&workspace, "../outside/secret.png", ImageReadMode::WorkspaceConfined).await;
        let trav_txt = read_image_source(&workspace, "../outside/secret.txt", ImageReadMode::WorkspaceConfined).await;
        let abs_out_png = read_image_source(&workspace, &outside_png.to_string_lossy(), ImageReadMode::WorkspaceConfined).await;
        let abs_out_txt = read_image_source(&workspace, &outside_txt.to_string_lossy(), ImageReadMode::WorkspaceConfined).await;
        let missing_out = read_image_source(&workspace, "../outside/does-not-exist.png", ImageReadMode::WorkspaceConfined).await;
        for resp in [&trav_png, &trav_txt, &abs_out_png, &abs_out_txt, &missing_out] {
            assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        }

        #[cfg(unix)]
        {
            let escape_link = workspace.join("escape.png");
            std::os::unix::fs::symlink(&outside_png, &escape_link).expect("symlink");
            let link_resp = read_image_source(&workspace, "escape.png", ImageReadMode::WorkspaceConfined).await;
            assert_eq!(link_resp.status(), StatusCode::NOT_FOUND);
        }

        let missing_ws = root.path().join("no-such-workspace");
        let missing_ws_resp = read_image_source(&missing_ws, "pic.png", ImageReadMode::WorkspaceConfined).await;
        assert_eq!(missing_ws_resp.status(), StatusCode::NOT_FOUND);
    }
}
