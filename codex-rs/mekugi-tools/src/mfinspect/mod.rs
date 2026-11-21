use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::ToolOutput;
use crate::common::clean_path;
use crate::common::path_to_slash;
use crate::common::quote_json_string;
use crate::sourcekind;
use crate::structure;

#[derive(Debug, Serialize)]
struct InspectionData {
    path: String,
    kind: String,
    language: Option<String>,
    size_bytes: u64,
    line_count: Option<usize>,
    parse_complete: bool,
    outline: Vec<structure::Entry>,
}

#[derive(Debug, Serialize)]
struct SuccessEnvelope {
    ok: bool,
    data: InspectionData,
    truncated: bool,
    truncation: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope<'a> {
    ok: bool,
    path: Option<&'a str>,
    error: ErrorDetail,
}

struct InspectFailure {
    code: &'static str,
    message: String,
}

fn inspect_display(s: &str) -> String {
    if s.chars().any(|ch| (ch as u32) < 32 || (ch as u32) == 127) {
        quote_json_string(s)
    } else {
        s.to_string()
    }
}

fn encode_failure(as_json: bool, path: Option<&str>, err: &InspectFailure) -> String {
    if as_json {
        let envelope = ErrorEnvelope {
            ok: false,
            path,
            error: ErrorDetail {
                code: err.code,
                message: err.message.clone(),
            },
        };
        let encoded = serde_json::to_string(&envelope).unwrap_or_else(|_| {
            format!(
                "{{\"ok\":false,\"path\":null,\"error\":{{\"code\":{:?},\"message\":{:?}}}}}",
                err.code, err.message
            )
        });
        format!("{encoded}\n")
    } else if let Some(path) = path {
        format!("mfinspect: {}: {}\n", inspect_display(path), err.message)
    } else {
        format!("mfinspect: {}\n", err.message)
    }
}

fn inspect_path(raw_path: &str) -> Result<InspectionData, InspectFailure> {
    if raw_path.is_empty() || raw_path.contains('\0') {
        return Err(InspectFailure {
            code: "usage",
            message: "mfinspect expects exactly one usable path".to_string(),
        });
    }
    let cleaned = clean_path(raw_path);
    let path_ref = Path::new(&cleaned);
    let meta = fs::metadata(path_ref).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            InspectFailure {
                code: "not_found",
                message: "path does not exist".to_string(),
            }
        } else {
            InspectFailure {
                code: "read",
                message: err.to_string(),
            }
        }
    })?;
    if !meta.is_file() {
        return Err(InspectFailure {
            code: "not_regular",
            message: "path is not a regular file".to_string(),
        });
    }

    let display_path = path_to_slash(path_ref);
    let Some(format) = sourcekind::classify(&cleaned).filter(|f| f.outline) else {
        return Ok(InspectionData {
            path: display_path,
            kind: "none".to_string(),
            language: None,
            size_bytes: meta.len(),
            line_count: None,
            parse_complete: true,
            outline: Vec::new(),
        });
    };

    let bytes = fs::read(path_ref).map_err(|err| InspectFailure {
        code: "read",
        message: err.to_string(),
    })?;
    let text = String::from_utf8(bytes).map_err(|_| InspectFailure {
        code: "not_utf8",
        message: "supported file is not valid UTF-8".to_string(),
    })?;
    let size_bytes = text.len() as u64;
    let parsed = structure::parse_outline(text, format).map_err(|err| InspectFailure {
        code: "parse",
        message: err,
    })?;

    let parse_complete = !parsed.entries.iter().any(|e| e.kind == "parse_error");
    Ok(InspectionData {
        path: display_path,
        kind: format.kind.to_string(),
        language: format.language.map(str::to_string),
        size_bytes,
        line_count: Some(parsed.lines.len()),
        parse_complete,
        outline: parsed.entries,
    })
}

fn compact_outline(data: &InspectionData) -> Vec<String> {
    let mut imports_start: Option<usize> = None;
    let mut imports_end: usize = 0;
    for entry in &data.outline {
        if entry.kind == "import" {
            imports_start = Some(match imports_start {
                Some(prev) => prev.min(entry.line),
                None => entry.line,
            });
            imports_end = imports_end.max(entry.end_line);
        }
    }

    let mut rows = Vec::new();
    for entry in &data.outline {
        if entry.kind == "import" {
            if let Some(start) = imports_start.take() {
                rows.push(format!("{start}-{imports_end} import\n"));
            }
            continue;
        }
        let name = match entry.kind.as_str() {
            "method" => format!("{}.{}", entry.receiver, entry.name),
            "json" => {
                let ptr = if entry.pointer.is_empty() {
                    "/"
                } else {
                    entry.pointer.as_str()
                };
                format!("{ptr} {}", entry.value_type)
            }
            _ => entry.name.clone(),
        };
        rows.push(format!(
            "{}-{} {} {}\n",
            entry.line,
            entry.end_line,
            entry.kind,
            inspect_display(&name)
        ));
    }
    if rows.is_empty() {
        rows.push("(no outline)\n".to_string());
    }
    rows
}

fn json_envelope(data: InspectionData) -> String {
    let envelope = SuccessEnvelope {
        ok: true,
        data,
        truncated: false,
        truncation: None,
    };
    let encoded = serde_json::to_string(&envelope).unwrap_or_default();
    format!("{encoded}\n")
}

pub fn execute(args: &[String]) -> ToolOutput {
    let mut as_json = false;
    for arg in args {
        if arg == "--" {
            break;
        }
        if arg == "--json" {
            as_json = true;
        }
    }

    let mut seen_json = false;
    let mut ended = false;
    let mut paths: Vec<&str> = Vec::new();

    for arg in args {
        if !ended && arg == "--" {
            ended = true;
            continue;
        }
        if !ended && arg == "--json" {
            if seen_json {
                return ToolOutput {
                    stdout: String::new(),
                    stderr: encode_failure(
                        as_json,
                        None,
                        &InspectFailure {
                            code: "usage",
                            message: "--json cannot repeat".to_string(),
                        },
                    ),
                    exit_code: 1,
                };
            }
            seen_json = true;
            continue;
        }
        if !ended && arg.starts_with("--preview-bytes") {
            return ToolOutput {
                stdout: String::new(),
                stderr: encode_failure(
                    as_json,
                    None,
                    &InspectFailure {
                        code: "usage",
                        message: "mfinspect does not accept source preview flags".to_string(),
                    },
                ),
                exit_code: 1,
            };
        }
        if !ended && (arg == "--max-tokens" || arg.starts_with("--max-tokens=")) {
            return ToolOutput {
                stdout: String::new(),
                stderr: encode_failure(
                    as_json,
                    None,
                    &InspectFailure {
                        code: "usage",
                        message: "unknown option \"--max-tokens\"".to_string(),
                    },
                ),
                exit_code: 1,
            };
        }
        if !ended && arg.starts_with('-') {
            return ToolOutput {
                stdout: String::new(),
                stderr: encode_failure(
                    as_json,
                    None,
                    &InspectFailure {
                        code: "usage",
                        message: format!("unknown option {arg:?}"),
                    },
                ),
                exit_code: 1,
            };
        }
        paths.push(arg.as_str());
    }

    if paths.is_empty() {
        return ToolOutput {
            stdout: String::new(),
            stderr: encode_failure(
                as_json,
                None,
                &InspectFailure {
                    code: "usage",
                    message: "mfinspect expects PATH [PATH ...]".to_string(),
                },
            ),
            exit_code: 1,
        };
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = 0;

    for &path in &paths {
        match inspect_path(path) {
            Ok(data) => {
                if as_json {
                    stdout.push_str(&json_envelope(data));
                } else {
                    if paths.len() > 1 {
                        let _ = writeln!(stdout, "--- {} ---", inspect_display(&data.path));
                    }
                    for row in compact_outline(&data) {
                        stdout.push_str(&row);
                    }
                }
            }
            Err(err) => {
                stderr.push_str(&encode_failure(as_json, Some(path), &err));
                exit_code = 1;
            }
        }
    }

    ToolOutput {
        stdout,
        stderr,
        exit_code,
    }
}

#[cfg(test)]
mod tests;
