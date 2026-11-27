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
const DETAIL_FILE_LIMIT: usize = 8 << 20;
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
    #[serde(default)]
    probe: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileBody {
    path: PathBuf,
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
        "attachment-image" | "file" => ATTACHMENT_IMAGE_BODY_LIMIT,
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
        "file" => handle_file(thread_processor, params).await,
        "image" => handle_image(thread_processor, params).await,
        "attachment-image" => handle_attachment_image(thread_processor, params).await,
        _ => error_response(StatusCode::NOT_FOUND, "Unknown resource endpoint."),
    }
}

#[tracing::instrument(skip_all)]
async fn handle_file(
    thread_processor: &ThreadRequestProcessor,
    params: ResourceParams,
) -> Response {
    let Some(body) = params.body else {
        return error_response(StatusCode::BAD_REQUEST, "A file path is required.");
    };
    let Ok(FileBody { path }) = serde_json::from_value(body) else {
        return error_response(StatusCode::BAD_REQUEST, "Invalid file request.");
    };
    if params.thread.is_empty() || path.as_os_str().is_empty() {
        return error_response(
            StatusCode::BAD_REQUEST,
            "A thread and file path are required.",
        );
    }
    let Ok(response) = thread_processor
        .thread_read_response_inner(ThreadReadParams {
            thread_id: params.thread,
            include_turns: false,
        })
        .await
    else {
        return error_response(
            StatusCode::NOT_FOUND,
            "The requested thread is unavailable.",
        );
    };
    let workspace = Path::new(response.thread.cwd.as_str());
    let Some(path) = resolve_detail_file(workspace, &path).await else {
        return error_response(
            StatusCode::NOT_FOUND,
            "The requested file is not available in this workspace.",
        );
    };
    let Ok((file, metadata)) = open_resource_file(&path).await else {
        return error_response(StatusCode::NOT_FOUND, "The requested file is unavailable.");
    };
    if !metadata.is_file() {
        return error_response(StatusCode::NOT_FOUND, "The requested file is unavailable.");
    };
    let mut bytes = Vec::new();
    if file
        .take((DETAIL_FILE_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .is_err()
    {
        return error_response(
            StatusCode::NOT_FOUND,
            "The requested file could not be read.",
        );
    }
    if bytes.len() > DETAIL_FILE_LIMIT {
        return error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "File details are limited to 8 MiB.",
        );
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "The requested file is not UTF-8 text.",
        );
    };
    if text.contains('\0') {
        return error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "The requested file is not UTF-8 text.",
        );
    }
    json_response(&serde_json::json!({ "text": text }))
}

#[tracing::instrument(skip_all)]
async fn resolve_detail_file(workspace: &Path, path: &Path) -> Option<PathBuf> {
    if !workspace.is_absolute() {
        return None;
    }
    let root = tokio::fs::canonicalize(workspace).await.ok()?;
    let path = tokio::fs::canonicalize(root.join(path)).await.ok()?;
    path.starts_with(&root).then_some(path)
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
    read_image_source(&workspace, &source).await
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
            for (image_index, candidate) in extract_item_image_sources(&item.payload)
                .into_iter()
                .enumerate()
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
    if request.path.is_empty() || request.path.starts_with("data:") || request.path.contains("://")
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
        if !path.is_absolute() || !tokio::fs::metadata(&path).await.is_ok_and(|m| m.is_dir()) {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Choose an existing absolute workspace directory.",
            );
        }
        path
    };
    if request.probe {
        match read_image_file(&workspace, &request.path, ImageFileRead::Probe).await {
            Ok(bytes) => json_response(&serde_json::json!({ "mime": detect_raster_mime(&bytes) })),
            Err(response) => response,
        }
    } else {
        read_image_source(&workspace, &request.path).await
    }
}

enum ImageFileRead {
    Probe,
    Preview,
}

#[tracing::instrument(skip_all)]
async fn open_resource_file(path: &Path) -> std::io::Result<(tokio::fs::File, std::fs::Metadata)> {
    let mut options = tokio::fs::OpenOptions::new();
    options.read(true);
    // A named pipe must not block open before we can check the file type.
    #[cfg(unix)]
    options.custom_flags(libc::O_NONBLOCK);
    let file = options.open(path).await?;
    let metadata = file.metadata().await?;
    Ok((file, metadata))
}

#[tracing::instrument(skip_all)]
async fn read_image_file(
    workspace: &Path,
    source: &str,
    mode: ImageFileRead,
) -> Result<Vec<u8>, Response> {
    let candidate = Path::new(source);
    let resolved = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        if !workspace.is_absolute() {
            return Err(error_response(
                StatusCode::BAD_REQUEST,
                "The image has no workspace.",
            ));
        }
        workspace.join(candidate)
    };
    let unavailable =
        || error_response(StatusCode::NOT_FOUND, "The attachment file is unavailable.");
    let (file, metadata) = open_resource_file(&resolved)
        .await
        .map_err(|_| unavailable())?;
    if matches!(mode, ImageFileRead::Probe) && metadata.is_dir() {
        return Ok(Vec::new());
    }
    if !metadata.is_file() {
        return Err(unavailable());
    }
    let limit = match mode {
        ImageFileRead::Probe => 12,
        ImageFileRead::Preview => UPLOAD_LIMIT + 1,
    };
    let mut bytes = Vec::new();
    file.take(limit as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| unavailable())?;
    Ok(bytes)
}

async fn read_image_source(workspace: &Path, source: &str) -> Response {
    let data = if let Some(rest) = source.strip_prefix("data:") {
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
        match read_image_file(workspace, source, ImageFileRead::Preview).await {
            Ok(bytes) => bytes,
            Err(response) => return response,
        }
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

    #[cfg(unix)]
    #[tokio::test]
    async fn resolves_workspace_file_links_without_following_links_outside_the_workspace() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let root = temp.path().join("workspace");
        let outside = temp.path().join("outside.txt");
        let linked = root.join("file.txt");
        std::fs::create_dir(&root).expect("create workspace");
        std::fs::write(&linked, "inside").expect("create workspace file");
        std::fs::write(&outside, "outside").expect("create separate file");
        std::os::unix::fs::symlink(&outside, root.join("escape.txt")).expect("link separate file");

        let canonical = linked.canonicalize().expect("resolve test file");
        assert_eq!(
            resolve_detail_file(&root, Path::new("file.txt")).await,
            Some(canonical.clone())
        );
        assert_eq!(resolve_detail_file(&root, &linked).await, Some(canonical));
        assert_eq!(
            resolve_detail_file(&root, Path::new("../outside.txt")).await,
            None
        );
        assert_eq!(
            resolve_detail_file(&root, Path::new("escape.txt")).await,
            None
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn attachment_probe_distinguishes_directories_from_named_pipes() {
        let temp = tempfile::tempdir().expect("create temp directory");
        let path = temp.path().join("pipe");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .expect("create named pipe")
                .success()
        );
        assert!(
            read_image_file(temp.path(), ".", ImageFileRead::Probe)
                .await
                .expect("directories are not images")
                .is_empty()
        );
        let error = read_image_file(temp.path(), "pipe", ImageFileRead::Probe)
            .await
            .expect_err("named pipes cannot be read as image attachments");
        assert_eq!(error.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn detects_supported_raster_images_only() {
        assert_eq!(
            detect_raster_mime(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some("image/png")
        );
        assert_eq!(
            detect_raster_mime(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg")
        );
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
}
