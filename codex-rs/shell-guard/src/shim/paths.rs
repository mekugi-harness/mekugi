use crate::protocol::SUDO_DIR_NAME;
use crate::protocol::VCS_DIR_NAME;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

pub(crate) fn same_file(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Preserves caller-selected `argv[0]` (e.g., from `exec -a` or `env --argv0`) when
/// it differs from the helper executable path.
pub(crate) fn forwarded_argv0(argv0: &str, target: &str) -> String {
    if let Ok(self_exe) = std::env::current_exe()
        && Path::new(argv0) != self_exe.as_path()
    {
        return argv0.to_owned();
    }
    target.to_owned()
}

/// Resolves the guard directory (if invoked through a symlink) and the real target
/// binary on disk, skipping self and any other session's `vcs-guard`/`sudo-guard` dirs.
pub(crate) fn guard_paths(
    name: &str,
    explicit: Option<&str>,
    default_path: bool,
) -> Result<(Option<PathBuf>, PathBuf), String> {
    let self_exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let self_info = fs::metadata(&self_exe).map_err(|err| err.to_string())?;

    let mut guard_dir = std::env::args_os()
        .next()
        .and_then(|raw| {
            let s = raw.to_string_lossy();
            s.contains('/')
                .then(|| Path::new(s.as_ref()).parent().map(Path::to_path_buf))
        })
        .flatten();

    if let Some(explicit_path) = explicit
        && !explicit_path.is_empty()
    {
        let path = Path::new(explicit_path);
        let base_ok = path.file_name().and_then(OsStr::to_str) == Some(name);
        let info = fs::metadata(path).map_err(|_| format!("{explicit_path}: command not found"))?;
        if !base_ok || !info.is_file() {
            return Err(format!("{explicit_path}: command not found"));
        }
        if !same_file(&info, &self_info) {
            return Ok((guard_dir, path.to_path_buf()));
        }
        // Path pointed into a guard directory; fall through to find the real binary.
    }

    let path_list: OsString = if default_path {
        let output = Command::new("/usr/bin/getconf")
            .arg("PATH")
            .output()
            .map_err(|err| format!("resolve standard utility path: {err}"))?;
        if !output.status.success() {
            return Err("resolve standard utility path failed".to_owned());
        }
        OsString::from(String::from_utf8_lossy(&output.stdout).trim())
    } else {
        std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/bin:/usr/bin"))
    };

    for entry in std::env::split_paths(&path_list) {
        let dir = if entry.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            entry
        };
        let candidate = dir.join(name);
        let Ok(info) = fs::metadata(&candidate) else {
            continue;
        };
        if !info.is_file() || info.permissions().mode() & 0o111 == 0 {
            continue;
        }
        if same_file(&info, &self_info) {
            if guard_dir.is_none() {
                guard_dir = Some(dir);
            }
            continue;
        }
        if let Some(dir_name) = dir.file_name().and_then(OsStr::to_str)
            && (dir_name == VCS_DIR_NAME || dir_name == SUDO_DIR_NAME)
        {
            continue;
        }
        let resolved = if candidate.to_string_lossy().contains('/') {
            candidate
        } else {
            PathBuf::from(".").join(candidate)
        };
        return Ok((guard_dir, resolved));
    }

    Err("command not found".to_owned())
}

/// Builds the child environment for a guarded command, applying `mise` shim loop
/// avoidance (`__MISE_SHIM_PATH` + removing other sessions' `vcs-guard` dirs from `PATH`)
/// when `real` resolves to a `mise` shim symlink.
pub(crate) fn guard_environment(real: &Path) -> Vec<(OsString, OsString)> {
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    let Ok(resolved) = fs::canonicalize(real) else {
        return env;
    };
    let real_base = real.file_name().and_then(OsStr::to_str);
    let resolved_base = resolved.file_name().and_then(OsStr::to_str);
    if real_base == Some("mise") || resolved_base != Some("mise") {
        return env;
    }
    let Ok(helper) = std::env::current_exe() else {
        return env;
    };
    let Ok(self_info) = fs::metadata(&helper) else {
        return env;
    };
    let Some(tool_name) = real.file_name() else {
        return env;
    };

    for (key, value) in &mut env {
        if key == OsStr::new("PATH") {
            let kept: Vec<PathBuf> = std::env::split_paths(value)
                .filter(|dir| {
                    if dir.file_name().and_then(OsStr::to_str) != Some(VCS_DIR_NAME) {
                        return true;
                    }
                    match fs::metadata(dir.join(tool_name)) {
                        Ok(info) => same_file(&info, &self_info),
                        Err(_) => true,
                    }
                })
                .collect();
            if let Ok(joined) = std::env::join_paths(kept) {
                *value = joined;
            }
        }
    }
    env.retain(|(k, _)| k != OsStr::new("__MISE_SHIM_PATH"));
    env.push((OsString::from("__MISE_SHIM_PATH"), helper.into_os_string()));
    env
}
