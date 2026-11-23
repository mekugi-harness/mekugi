//! Pure native shell-guard engine for Mekugi: remote-write VCS approvals,
//! `sudo` command approval and private askpass, and `gpg`/`gpg2` loopback
//! passphrase prompts.

pub mod broker;
pub mod classifier;
pub mod protocol;
pub mod rewriter;
pub mod secret;
#[cfg(unix)]
pub mod shim;

pub use broker::ShellGuardBroker;
pub use broker::ShellGuardBrokerConfig;
pub use broker::ShellGuardBrokerDecision;
pub use broker::ShellGuardHandler;
pub use broker::ShellGuardResolvedReason;
pub use protocol::ASKPASS_NAME;
pub use protocol::CLIENT_TIMEOUT;
pub use protocol::GuardKind;
pub use protocol::MAX_MESSAGE_BYTES;
pub use protocol::PINNED_EXE_NAME;
pub use protocol::SOCKET_ENV_VAR;
pub use protocol::SUDO_COMMAND_ENV_VAR;
pub use protocol::SUDO_DIR_ENV_VAR;
pub use protocol::SUDO_DIR_NAME;
pub use protocol::SUDO_TOOLS;
pub use protocol::ShimReply;
pub use protocol::ShimRequest;
pub use protocol::TIMEOUT;
pub use protocol::VCS_DIR_ENV_VAR;
pub use protocol::VCS_DIR_NAME;
pub use protocol::VCS_TOOLS;
pub use rewriter::RewriteConfig;
pub use rewriter::rewrite_script;
pub use secret::SecretString;

/// Stage guard-only aliases when an interactive session first uses its shell.
#[cfg(unix)]
pub fn stage_aliases(dir: &std::path::Path) -> std::io::Result<()> {
    use std::fs;
    use std::io;
    use std::io::Seek;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::fs::symlink;

    let pinned = dir.join(PINNED_EXE_NAME);
    if !pinned.try_exists()? {
        // Share one atomic copy across threads when the arg0 directory
        // lives on a different filesystem or hard links are unavailable.
        let lock = fs::File::create(dir.join(".codex-image.lock"))?;
        lock.lock()?;
        if !pinned.try_exists()? {
            let mut image = fs::File::open(dir.join(".codex-image"))?;
            image.rewind()?;
            let mut copy = tempfile::NamedTempFile::new_in(dir)?;
            io::copy(&mut image, &mut copy)?;
            copy.as_file()
                .set_permissions(fs::Permissions::from_mode(0o700))?;
            copy.persist(&pinned)?;
        }
    }
    for (name, tools) in [(VCS_DIR_NAME, VCS_TOOLS), (SUDO_DIR_NAME, SUDO_TOOLS)] {
        let aliases = dir.join(name);
        fs::create_dir_all(&aliases)?;
        for tool in tools
            .iter()
            .copied()
            .chain((name == SUDO_DIR_NAME).then_some(ASKPASS_NAME))
        {
            if let Err(err) = symlink(&pinned, aliases.join(tool))
                && err.kind() != io::ErrorKind::AlreadyExists
            {
                return Err(err);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod broker_tests;
#[cfg(test)]
mod classifier_tests;
#[cfg(test)]
mod rewriter_tests;
