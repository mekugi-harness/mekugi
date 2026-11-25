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
use codex_app_server_protocol::ImageReference;
use codex_app_server_protocol::ThreadItemPayload;
use codex_app_server_protocol::ThreadReadParams;
use codex_app_server_protocol::UserInput;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use tokio::io::AsyncReadExt;

use crate::message_processor::MessageProcessor;

const PROTOCOL_HEADER: &str = "Mekugi-Protocol";
const PROTOCOL_VERSION: &str = "9";
const UPLOAD_LIMIT: usize = 32 << 20;
const DEFAULT_RESOURCE_BODY_LIMIT: usize = 1024;
const ATTACHMENT_IMAGE_BODY_LIMIT: usize = 64 << 10;
const ENVELOPE_OVERHEAD: usize = 64 << 10;

#[derive(Clone)]
struct ResourceState {
    processor: Arc<OnceLock<Arc<MessageProcessor>>>,
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

pub(crate) fn router(processor: Arc<OnceLock<Arc<MessageProcessor>>>) -> Router {
    Router::new()
        .route("/api/version", get(version_handler))
        .route("/api/resources/{kind}", post(resource_handler))
        .with_state(ResourceState { processor })
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

async fn version_handler(headers: HeaderMap) -> Response {
    if let Err(response) = check_protocol_header(&headers) {
        return response;
    }
    json_response(&serde_json::json!({
        "ready": true,
        "version": PROTOCOL_VERSION,
    }))
}

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
    let Some(processor) = state.processor.get() else {
        return error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "App server is still starting.",
        );
    };
    match kind.as_str() {
        "content" => handle_content(processor, params).await,
        "image" => handle_image(processor, params).await,
        "attachment-image" => handle_attachment_image(processor, params).await,
        _ => error_response(StatusCode::NOT_FOUND, "Unknown resource endpoint."),
    }
}

async fn handle_content(processor: &MessageProcessor, params: ResourceParams) -> Response {
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
    let response = match processor
        .thread_processor()
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

async fn handle_image(processor: &MessageProcessor, params: ResourceParams) -> Response {
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
    let response = match processor
        .thread_processor()
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
    let mut matched_source: Option<String> = None;
    'outer: for turn in &response.thread.turns {
        for item in &turn.items {
            let ThreadItemPayload::UserMessage { id, content, .. } = &item.payload else {
                continue;
            };
            if let Some(ref expected_item_id) = image_body.item_id
                && id != expected_item_id
            {
                continue;
            }
            let mut image_index = 0usize;
            for input in content {
                let candidate = match input {
                    UserInput::LocalImage { path, .. } => path.to_string_lossy().into_owned(),
                    UserInput::Image {
                        image: ImageReference::Inline { url },
                        ..
                    } => url.clone(),
                    _ => continue,
                };
                let index_ok = image_body.index.is_none_or(|idx| idx == image_index);
                let identity_ok =
                    image_body.image.is_empty() || image_identity(&candidate) == image_body.image;
                if index_ok && identity_ok {
                    matched_source = Some(candidate);
                    break 'outer;
                }
                image_index += 1;
            }
        }
    }
    let Some(source) = matched_source else {
        return error_response(
            StatusCode::NOT_FOUND,
            "This attached image is no longer available.",
        );
    };
    read_image_source(&workspace, &source).await
}

async fn handle_attachment_image(processor: &MessageProcessor, params: ResourceParams) -> Response {
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
        let response = match processor
            .thread_processor()
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
    read_image_source(&workspace, &request.path).await
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
        let candidate = Path::new(source);
        let resolved = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            if !workspace.is_absolute() {
                return error_response(StatusCode::BAD_REQUEST, "The image has no workspace.");
            }
            workspace.join(candidate)
        };
        let mut file = match tokio::fs::File::open(&resolved).await {
            Ok(file) => file,
            Err(_) => {
                return error_response(
                    StatusCode::NOT_FOUND,
                    "The attached image file is unavailable.",
                );
            }
        };
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
}
