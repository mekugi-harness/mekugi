use std::path::Path;

/// Sentinel marking a shell word whose value is only known at runtime.
pub(crate) const DYNAMIC: &str = "\0";

/// Result of unwrapping command-dispatching wrappers (`env`, `command`, `timeout`, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnwrappedCommand {
    pub index: usize,
    pub default_path: bool,
    pub env_cleared_path: bool,
}

fn wrapper_operands(wrapper: &str) -> Option<&'static [&'static str]> {
    match wrapper {
        "env" => Some(&["-u", "--unset", "-C", "--chdir", "-a", "--argv0"]),
        "command" => Some(&[]),
        "exec" => Some(&["-a"]),
        "nohup" => Some(&[]),
        "time" => Some(&[]),
        "builtin" => Some(&[]),
        "timeout" => Some(&["-k", "--kill-after", "-s", "--signal"]),
        "nice" => Some(&["-n", "--adjustment"]),
        "xargs" => Some(&[
            "-a",
            "--arg-file",
            "-d",
            "--delimiter",
            "-E",
            "-I",
            "-L",
            "-n",
            "--max-args",
            "-P",
            "--max-procs",
            "-s",
            "--max-chars",
        ]),
        _ => None,
    }
}

pub(crate) fn basename(path: &str) -> &str {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
}

/// Unwraps wrapper commands without confusing option operands with the target executable.
/// Lookup-only invocations (`command -v`, `command -V`) return `None`.
pub(crate) fn command_word(words: &[String]) -> Option<UnwrappedCommand> {
    let mut default_path = false;
    let mut env_cleared_path = false;
    let mut i = 0;

    while i < words.len() {
        let value = &words[i];
        if value.contains(DYNAMIC) {
            return Some(UnwrappedCommand {
                index: i,
                default_path,
                env_cleared_path,
            });
        }
        let wrapper = basename(value);
        let Some(operands) = wrapper_operands(wrapper) else {
            return Some(UnwrappedCommand {
                index: i,
                default_path,
                env_cleared_path,
            });
        };
        i += 1;
        let mut options_done = false;
        while i < words.len() {
            let arg = &words[i];
            if wrapper == "env" && !arg.starts_with('-') && arg.contains('=') {
                if arg.starts_with("PATH=") {
                    env_cleared_path = true;
                }
                i += 1;
                continue;
            }
            if arg.contains(DYNAMIC) && wrapper != "timeout" {
                return Some(UnwrappedCommand {
                    index: i,
                    default_path,
                    env_cleared_path,
                });
            }
            if !options_done && arg == "--" {
                i += 1;
                if wrapper != "env" {
                    break;
                }
                options_done = true;
                continue;
            }
            if options_done || !arg.starts_with('-') || arg == "-" {
                break;
            }
            if wrapper == "command" && (arg.contains('v') || arg.contains('V')) {
                return None;
            }
            if wrapper == "command" && arg.contains('p') {
                default_path = true;
            }
            if wrapper == "env" {
                if arg == "-S" || arg.starts_with("--split-string") {
                    return None;
                }
                if arg == "-i"
                    || arg == "--ignore-environment"
                    || (!arg.starts_with("--") && arg.contains('i'))
                {
                    env_cleared_path = true;
                }
                if arg == "--unset=PATH" || arg == "-uPATH" {
                    env_cleared_path = true;
                }
                if (arg == "-u" || arg == "--unset")
                    && words.get(i + 1).is_some_and(|next| next == "PATH")
                {
                    env_cleared_path = true;
                }
            }
            let takes_operand = operands.contains(&arg.as_str());
            i += 1;
            if takes_operand {
                i += 1;
            }
        }
        if wrapper == "timeout" {
            i += 1; // Duration operand.
        }
    }
    None
}
