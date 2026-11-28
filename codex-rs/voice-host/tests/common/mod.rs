//! Creates the voice helper subprocess for integration tests.

use anyhow::Result;
use codex_utils_cargo_bin::cargo_bin;
use tokio::process::Command;

pub(crate) fn voice_host_command() -> Result<Command> {
    Ok(Command::new(cargo_bin("codex-voice-host")?))
}
