use std::collections::HashMap;
use std::ffi::{OsStr, OsString};

/// Accepts one GPG `--command-fd` response without framing or NUL characters.
pub fn valid_passphrase(value: &str) -> bool {
    !value.contains(['\r', '\n', '\0'])
}

/// Result of classifying GPG command-line arguments against its dumped option table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpgParsedArgs {
    pub forward: Vec<OsString>,
    pub status_fd: Option<i32>,
    pub interactive: bool,
}

/// Parses `gpg --no-options --dump-option-table` output (`name:code:flags:description`).
/// The low three flag bits indicate argument type; bit 3 marks an optional argument.
pub fn parse_option_table(data: &str) -> Result<HashMap<String, i32>, String> {
    let mut options = HashMap::new();
    for line in data.split('\n') {
        let mut parts = line.splitn(4, ':');
        let (Some(name), Some(code), Some(flags_str), Some(_desc)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let flags: i32 = flags_str
            .parse()
            .map_err(|err| format!("invalid GPG option flags: {err}"))?;
        options.insert(format!("--{name}"), flags);
        if let Ok(short) = code.parse::<u32>()
            && short > 0
            && short < 128
            && let Some(ch) = char::from_u32(short)
        {
            options.insert(format!("-{ch}"), flags);
        }
    }
    if !options.contains_key("--pinentry-mode") {
        return Err("missing GPG option table".to_owned());
    }
    Ok(options)
}

/// Preserves native argv (`interactive = false`) on ambiguous/unknown options or
/// caller-owned authentication/status options. Strips `--status-fd` from `forward`
/// and records it in `status_fd` when interactive loopback handling applies.
pub fn parse_arguments(
    args: &[impl AsRef<OsStr>],
    options: &HashMap<String, i32>,
) -> GpgParsedArgs {
    let fallback = || GpgParsedArgs {
        forward: args.iter().map(|a| a.as_ref().to_owned()).collect(),
        status_fd: None,
        interactive: false,
    };

    let mut forward = Vec::with_capacity(args.len());
    let mut status_fd: Option<i32> = None;
    let mut i = 0;

    while i < args.len() {
        let start = i;
        let text = args[i].as_ref().to_string_lossy();
        let arg = text.as_ref();
        if arg == "--" || !arg.starts_with('-') || arg == "-" {
            forward.extend(args[i..].iter().map(|a| a.as_ref().to_owned()));
            return GpgParsedArgs {
                forward,
                status_fd,
                interactive: true,
            };
        }
        let (raw_name, mut value, attached) = match arg.split_once('=') {
            Some((before, after)) => (before, after, true),
            None => (arg, "", false),
        };
        if arg.starts_with("--") {
            let resolved_name = if options.contains_key(raw_name) {
                raw_name
            } else {
                let mut found: Option<&str> = None;
                for candidate in options.keys() {
                    if candidate.starts_with(raw_name) {
                        if found.is_some() {
                            return fallback();
                        }
                        found = Some(candidate.as_str());
                    }
                }
                match found {
                    Some(name) => name,
                    None => return fallback(),
                }
            };
            let Some(&flags) = options.get(resolved_name) else {
                return fallback();
            };
            if matches!(
                resolved_name,
                "--pinentry-mode"
                    | "--passphrase"
                    | "--passphrase-fd"
                    | "--passphrase-file"
                    | "--command-fd"
                    | "--command-file"
                    | "--status-file"
                    | "--options"
            ) {
                return fallback();
            }
            let takes_arg = flags & 7 != 0;
            let optional_arg = flags & 8 != 0;
            if takes_arg
                && !attached
                && (!optional_arg
                    || args
                        .get(i + 1)
                        .is_some_and(|next| !next.as_ref().as_encoded_bytes().starts_with(b"-")))
            {
                i += 1;
                let Some(next) = args.get(i) else {
                    return fallback();
                };
                value = next.as_ref().to_str().unwrap_or("");
            }
            if resolved_name == "--status-fd" {
                let Ok(fd) = value.parse::<i32>() else {
                    return fallback();
                };
                if fd < 0 {
                    return fallback();
                }
                status_fd = Some(fd);
                i += 1;
                continue;
            }
        } else {
            let bytes = arg.as_bytes();
            let mut j = 1;
            while j < bytes.len() {
                let key = format!("-{}", bytes[j] as char);
                let Some(&flags) = options.get(&key) else {
                    return fallback();
                };
                if flags & 7 == 0 {
                    j += 1;
                    continue;
                }
                let optional_arg = flags & 8 != 0;
                if j + 1 == bytes.len()
                    && (!optional_arg
                        || args.get(i + 1).is_some_and(|next| {
                            !next.as_ref().as_encoded_bytes().starts_with(b"-")
                        }))
                {
                    i += 1;
                }
                if i >= args.len() {
                    return fallback();
                }
                break;
            }
        }
        forward.extend(args[start..=i].iter().map(|a| a.as_ref().to_owned()));
        i += 1;
    }

    GpgParsedArgs {
        forward,
        status_fd,
        interactive: true,
    }
}
