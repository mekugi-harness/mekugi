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
    let Some((file, metadata)) = open_confined_resource_file(workspace, &path).await else {
        return error_response(
            StatusCode::NOT_FOUND,
            "The requested file is not available in this workspace.",
        );
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
async fn canonicalize_workspace_dir(workspace: &Path) -> Option<PathBuf> {
    if !workspace.is_absolute() {
        return None;
    }
    let root = tokio::fs::canonicalize(workspace).await.ok()?;
    tokio::fs::metadata(&root)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
        .then_some(root)
}

#[tracing::instrument(skip_all)]
async fn resolve_workspace_path(workspace: &Path, candidate: &Path) -> Option<(PathBuf, PathBuf)> {
    let root = canonicalize_workspace_dir(workspace).await?;
    let target = tokio::fs::canonicalize(root.join(candidate)).await.ok()?;
    target.starts_with(&root).then_some((root, target))
}

#[cfg(test)]
#[tracing::instrument(skip_all)]
async fn resolve_detail_file(workspace: &Path, path: &Path) -> Option<PathBuf> {
    resolve_workspace_path(workspace, path)
        .await
        .map(|(_, target)| target)
}

#[tracing::instrument(skip_all)]
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

#[tracing::instrument(skip_all)]
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
    read_published_image_source(&workspace, &source).await
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

#[tracing::instrument(skip_all)]
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
        let Some(workspace) =
            canonicalize_workspace_dir(Path::new(response.thread.cwd.as_str())).await
        else {
            return attachment_unavailable_response();
        };
        workspace
    } else {
        let Some(workspace) = thread_processor
            .admit_attachment_workspace(Path::new(&request.workspace))
            .await
        else {
            return error_response(
                StatusCode::BAD_REQUEST,
                "Choose an admitted workspace directory.",
            );
        };
        workspace
    };
    let mode = if request.probe {
        ImageFileRead::Probe
    } else {
        ImageFileRead::Preview
    };
    match read_workspace_image_file(&workspace, Path::new(&request.path), mode).await {
        Ok(bytes) if request.probe => {
            json_response(&serde_json::json!({ "mime": detect_raster_mime(&bytes) }))
        }
        Ok(bytes) => raster_image_response(bytes),
        Err(response) => response,
    }
}

enum ImageFileRead {
    Probe,
    Preview,
}

fn attachment_unavailable_response() -> Response {
    error_response(StatusCode::NOT_FOUND, "The attachment file is unavailable.")
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

#[cfg(unix)]
fn openat_component(
    dir: &std::os::fd::OwnedFd,
    segment: &std::ffi::OsStr,
    flags: libc::c_int,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::fd::FromRawFd;
    use std::os::fd::OwnedFd;
    use std::os::unix::ffi::OsStrExt;

    let name = CString::new(segment.as_bytes())
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: `dir` is a valid open directory descriptor and `name` is NUL-terminated.
    let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` is a newly created file descriptor returned by `openat`.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn directory_access_flags() -> libc::c_int {
    libc::O_PATH
}

#[cfg(target_vendor = "apple")]
fn directory_access_flags() -> libc::c_int {
    libc::O_SEARCH
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android", target_vendor = "apple"))
))]
fn directory_access_flags() -> libc::c_int {
    libc::O_RDONLY
}

#[cfg(unix)]
fn open_confined_unix(
    canonical_root: &Path,
    relative: &Path,
) -> std::io::Result<(tokio::fs::File, std::fs::Metadata)> {
    use std::os::fd::FromRawFd;
    use std::os::fd::OwnedFd;

    let dir_flags = directory_access_flags() | libc::O_DIRECTORY | libc::O_CLOEXEC;
    // SAFETY: `c"/"` is a static NUL-terminated path and `dir_flags` opens a directory descriptor.
    let root_fd = unsafe { libc::open(c"/".as_ptr(), dir_flags) };
    if root_fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `root_fd` is a newly created file descriptor returned by `open`.
    let mut current = unsafe { OwnedFd::from_raw_fd(root_fd) };

    let step_flags = dir_flags | libc::O_NOFOLLOW;
    let mut saw_root = false;
    for component in canonical_root.components() {
        match component {
            std::path::Component::RootDir => saw_root = true,
            std::path::Component::Normal(segment) => {
                current = openat_component(&current, segment, step_flags)?;
            }
            std::path::Component::CurDir
            | std::path::Component::ParentDir
            | std::path::Component::Prefix(_) => {
                return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
            }
        }
    }
    if !saw_root {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    }

    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(segment) => components.push(segment),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
            }
        }
    }

    let final_fd = if let Some((last, parents)) = components.split_last() {
        for segment in parents {
            current = openat_component(&current, segment, step_flags)?;
        }
        // A named pipe must not block open before we check the file type.
        openat_component(
            &current,
            last,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )?
    } else {
        current
    };

    let file = std::fs::File::from(final_fd);
    let metadata = file.metadata()?;
    Ok((tokio::fs::File::from_std(file), metadata))
}

#[tracing::instrument(skip_all)]
async fn open_confined_resource_file(
    workspace: &Path,
    candidate: &Path,
) -> Option<(tokio::fs::File, std::fs::Metadata)> {
    let (root, target) = resolve_workspace_path(workspace, candidate).await?;
    #[cfg(unix)]
    {
        let relative = target.strip_prefix(&root).ok()?.to_path_buf();
        tokio::task::spawn_blocking(move || open_confined_unix(&root, &relative))
            .await
            .ok()?
            .ok()
    }
    #[cfg(not(unix))]
    {
        let _ = root;
        open_resource_file(&target).await.ok()
    }
}

#[tracing::instrument(skip_all)]
async fn read_image_bytes(
    file: tokio::fs::File,
    metadata: std::fs::Metadata,
    mode: ImageFileRead,
) -> Result<Vec<u8>, Response> {
    if matches!(mode, ImageFileRead::Probe) && metadata.is_dir() {
        return Ok(Vec::new());
    }
    if !metadata.is_file() {
        return Err(attachment_unavailable_response());
    }
    let limit = match mode {
        ImageFileRead::Probe => 12,
        ImageFileRead::Preview => UPLOAD_LIMIT + 1,
    };
    let mut bytes = Vec::new();
    file.take(limit as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| attachment_unavailable_response())?;
    Ok(bytes)
}

#[tracing::instrument(skip_all)]
async fn read_workspace_image_file(
    workspace: &Path,
    candidate: &Path,
    mode: ImageFileRead,
) -> Result<Vec<u8>, Response> {
    let (file, metadata) = open_confined_resource_file(workspace, candidate)
        .await
        .ok_or_else(attachment_unavailable_response)?;
    read_image_bytes(file, metadata, mode).await
}

#[tracing::instrument(skip_all)]
async fn read_published_image_file(workspace: &Path, source: &str) -> Result<Vec<u8>, Response> {
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
    let (file, metadata) = open_resource_file(&resolved)
        .await
        .map_err(|_| attachment_unavailable_response())?;
    read_image_bytes(file, metadata, ImageFileRead::Preview).await
}

#[tracing::instrument(skip_all)]
async fn read_published_image_source(workspace: &Path, source: &str) -> Response {
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
        match read_published_image_file(workspace, source).await {
            Ok(bytes) => bytes,
            Err(response) => return response,
        }
    };
    raster_image_response(data)
}

fn raster_image_response(data: Vec<u8>) -> Response {
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
#[path = "http_resources_tests.rs"]
mod tests;
