const SUDO_LONG_OPTIONS: &[&str] = &[
    "auth-type",
    "login-class",
    "askpass",
    "background",
    "bell",
    "close-from",
    "chdir",
    "preserve-env",
    "edit",
    "group",
    "set-home",
    "help",
    "host",
    "login",
    "remove-timestamp",
    "reset-timestamp",
    "list",
    "non-interactive",
    "no-update",
    "preserve-groups",
    "prompt",
    "chroot",
    "role",
    "stdin",
    "shell",
    "type",
    "command-timeout",
    "other-user",
    "user",
    "version",
    "validate",
];

/// Reports whether `sudo` should use Mekugi's askpass helper (excludes explicit
/// `-S`/`--stdin` and `-A`/`--askpass` modes).
pub fn interactive(args: &[String]) -> bool {
    applicable(args, /*password*/ true)
}

/// Reports whether `sudo` requires command approval (excludes non-interactive,
/// listing, informational, and timestamp-only modes).
pub fn command_approval(args: &[String]) -> bool {
    applicable(args, /*password*/ false)
}

/// Accepts one askpass answer without line-framing characters.
pub fn valid_password(password: &str) -> bool {
    !password.contains(['\r', '\n'])
}

fn applicable(args: &[String], password: bool) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg.contains('=') && !arg.starts_with('-') {
            i += 1;
            continue;
        }
        if arg == "--" || !arg.starts_with('-') || arg == "-" {
            break;
        }
        if arg.starts_with("--") {
            let (prefix, attached) = match arg.split_once('=') {
                Some((before, _)) => (before, true),
                None => (arg.as_str(), false),
            };
            let Some(prefix_body) = prefix.strip_prefix("--") else {
                return false;
            };
            let name = if SUDO_LONG_OPTIONS.contains(&prefix_body) {
                prefix_body
            } else {
                let mut matches = SUDO_LONG_OPTIONS
                    .iter()
                    .filter(|option| option.starts_with(prefix_body));
                match (matches.next(), matches.next()) {
                    (Some(option), None) => option,
                    _ => return false,
                }
            };
            match name {
                "list" | "help" | "version" | "remove-timestamp" | "non-interactive" => {
                    return false;
                }
                "stdin" | "askpass" if password => return false,
                "auth-type" | "login-class" | "close-from" | "chdir" | "group" | "host"
                | "prompt" | "chroot" | "role" | "type" | "command-timeout" | "other-user"
                | "user"
                    if !attached =>
                {
                    i += 1;
                }
                _ => {}
            }
            i += 1;
            continue;
        }
        let bytes = arg.as_bytes();
        let mut j = 1;
        while j < bytes.len() {
            match bytes[j] {
                b'l' | b'V' | b'K' | b'n' => return false,
                b'S' | b'A' if password => return false,
                b'a' | b'c' | b'C' | b'D' | b'g' | b'h' | b'p' | b'R' | b'r' | b't' | b'T'
                | b'U' | b'u' => {
                    if j + 1 == bytes.len() {
                        i += 1;
                    }
                    break;
                }
                _ => {}
            }
            j += 1;
        }
        i += 1;
    }
    !args.is_empty() && !(args.len() == 1 && (args[0] == "-k" || args[0] == "-h"))
}
