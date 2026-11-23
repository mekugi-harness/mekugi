use crate::protocol::CLIENT_TIMEOUT;
use crate::protocol::MAX_MESSAGE_BYTES;
use crate::protocol::ShimReply;
use crate::protocol::ShimRequest;
use std::io::Read;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;
use zeroize::Zeroizing;

const UNAVAILABLE: &str = "Mekugi approval is unavailable";

/// Sends one guard request over the session-private Unix domain socket and waits
/// synchronously for the broker's reply. Keeps the write half open while waiting
/// so peer-EOF detection in the broker notices if this process is killed.
pub(crate) fn send_request(
    socket_path: Option<&Path>,
    request: &ShimRequest,
) -> Result<ShimReply, String> {
    let Some(path) = socket_path else {
        return Err(UNAVAILABLE.to_owned());
    };
    let mut stream = UnixStream::connect(path).map_err(|_| UNAVAILABLE.to_owned())?;
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_read_timeout(Some(CLIENT_TIMEOUT));

    let mut payload =
        serde_json::to_vec(request).map_err(|_| "invalid approval request".to_owned())?;
    payload.push(b'\n');
    if payload.len() > MAX_MESSAGE_BYTES {
        return Err("invalid approval request".to_owned());
    }
    stream
        .write_all(&payload)
        .and_then(|()| stream.flush())
        .map_err(|_| UNAVAILABLE.to_owned())?;

    // The broker closes this one-request connection after writing its reply.
    // Read directly into the wiped buffer; a BufReader retains a second copy.
    let mut encoded = Zeroizing::new(Vec::new());
    stream
        .take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_to_end(&mut encoded)
        .map_err(|_| "no valid answer from Mekugi".to_owned())?;
    if encoded.len() > MAX_MESSAGE_BYTES {
        return Err("no valid answer from Mekugi".to_owned());
    }
    serde_json::from_slice(&encoded).map_err(|_| "no valid answer from Mekugi".to_owned())
}
