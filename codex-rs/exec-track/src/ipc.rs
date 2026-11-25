//! Unix-domain socket framing for interpose-based write observation (macOS)
//! and cross-platform unit testing.

#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Mutex;

use crate::persist::ObservationState;

/// Parse and handle a single framed message (`W<path>\0` or `S<sip_path>\0`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn handle_framed_message(buf: &[u8], state: &Mutex<ObservationState>) {
    let Some((&kind, rest)) = buf.split_first() else {
        return;
    };
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    let payload = &rest[..end];
    if payload.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(payload);
    let mut guard = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match kind {
        b'W' => guard.record_pre_write(Path::new(text.as_ref())),
        b'S' => guard.record_partial_reason(format!("executed SIP-protected binary: {text}")),
        _ => {}
    }
}

/// Read one NUL-terminated message from `stream`, apply it to `state`, and send 1-byte ACK.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn serve_unix_message(mut stream: UnixStream, state: &Mutex<ObservationState>) {
    let mut buf = Vec::with_capacity(256);
    let mut byte = [0u8; 1];
    while buf.len() < 8192 {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                buf.push(byte[0]);
                if byte[0] == 0 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    handle_framed_message(&buf, state);
    let _ = stream.write_all(&[1u8]);
}
