use std::collections::VecDeque;
use std::io::Read;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;

use crate::ToolOutput;
use crate::common::to_valid_utf8_collapsed;

const MAX_OUTPUT_TOKENS: usize = 15_500;
const MAX_TOKENS_ARGUMENT_ERROR: &str =
    "--max-tokens requires one positive integer and cannot repeat";
const RUN_USAGE_ERROR: &str =
    "expected -n N or --max-tokens N, optionally --head or --tail, then [--] COMMAND [ARG...]";

#[derive(Clone, Debug, Eq, PartialEq)]
struct RunOptions {
    lines: Option<u64>,
    tokens: Option<usize>,
    tail: bool,
}

fn is_canonical_positive(raw: &str) -> bool {
    !raw.is_empty() && !raw.starts_with('0') && raw.bytes().all(|b| b.is_ascii_digit())
}

fn parse_max_tokens(raw: &str) -> Result<usize, &'static str> {
    if !is_canonical_positive(raw) {
        return Err(MAX_TOKENS_ARGUMENT_ERROR);
    }
    let val = raw.parse::<usize>().unwrap_or(MAX_OUTPUT_TOKENS);
    Ok(val.min(MAX_OUTPUT_TOKENS))
}

fn parse_options(args: &[String]) -> Result<(RunOptions, Vec<String>), String> {
    let mut options = RunOptions {
        lines: None,
        tokens: None,
        tail: true,
    };
    let mut direction_set = false;
    let mut i = 0usize;

    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(val) = arg.strip_prefix("--max-tokens=") {
            if options.tokens.is_some() {
                return Err(MAX_TOKENS_ARGUMENT_ERROR.to_string());
            }
            options.tokens = Some(parse_max_tokens(val).map_err(str::to_string)?);
            i += 1;
            continue;
        }
        match arg {
            "--max-tokens" => {
                if i + 1 >= args.len() || options.tokens.is_some() {
                    return Err(MAX_TOKENS_ARGUMENT_ERROR.to_string());
                }
                options.tokens = Some(parse_max_tokens(&args[i + 1]).map_err(str::to_string)?);
                i += 2;
            }
            "-n" => {
                if i + 1 >= args.len() || options.lines.is_some() {
                    return Err("-n requires one positive integer and cannot repeat".to_string());
                }
                let raw = args[i + 1].as_str();
                if !is_canonical_positive(raw) {
                    return Err("-n requires one positive integer".to_string());
                }
                let parsed = raw
                    .parse::<u64>()
                    .map_err(|_| "-n requires one positive integer".to_string())?;
                options.lines = Some(parsed);
                i += 2;
            }
            "--head" | "--tail" => {
                if direction_set {
                    return Err("specify --head or --tail at most once".to_string());
                }
                direction_set = true;
                options.tail = arg == "--tail";
                i += 1;
            }
            "--" => {
                if (options.tokens.is_none() && options.lines.is_none())
                    || i + 1 >= args.len()
                    || args[i + 1].is_empty()
                {
                    return Err(RUN_USAGE_ERROR.to_string());
                }
                return Ok((options, args[i + 1..].to_vec()));
            }
            _ => {
                if !arg.starts_with('-')
                    && !arg.is_empty()
                    && (options.tokens.is_some() || options.lines.is_some())
                {
                    return Ok((options, args[i..].to_vec()));
                }
                return Err(RUN_USAGE_ERROR.to_string());
            }
        }
    }
    Err(RUN_USAGE_ERROR.to_string())
}

/// Bounded streaming byte ring / head window for token-bounded capture.
struct ByteWindow {
    buf: Vec<u8>,
    limit: usize,
    start: usize,
    size: usize,
    tail: bool,
    omitted: bool,
}

impl ByteWindow {
    fn new(limit: usize, tail: bool) -> Self {
        Self {
            buf: vec![0u8; limit],
            limit,
            start: 0,
            size: 0,
            tail,
            omitted: false,
        }
    }

    fn write(&mut self, data: &[u8]) {
        if self.limit == 0 {
            if !data.is_empty() {
                self.omitted = true;
            }
            return;
        }
        if !self.tail {
            let space = self.limit.saturating_sub(self.size);
            let accepted = data.len().min(space);
            self.buf[self.size..self.size + accepted].copy_from_slice(&data[..accepted]);
            self.size += accepted;
            if accepted < data.len() {
                self.omitted = true;
            }
            return;
        }
        if data.len() >= self.limit {
            if self.size > 0 || data.len() > self.limit {
                self.omitted = true;
            }
            let tail_slice = &data[data.len() - self.limit..];
            self.buf.copy_from_slice(tail_slice);
            self.start = 0;
            self.size = self.limit;
            return;
        }
        let excess = (self.size + data.len()).saturating_sub(self.limit);
        if excess > 0 {
            self.omitted = true;
            self.start = (self.start + excess) % self.limit;
            self.size -= excess;
        }
        let end = (self.start + self.size) % self.limit;
        let first = data.len().min(self.limit - end);
        self.buf[end..end + first].copy_from_slice(&data[..first]);
        let rem = data.len() - first;
        if rem > 0 {
            self.buf[..rem].copy_from_slice(&data[first..]);
        }
        self.size += data.len();
    }

    fn take_bytes(&mut self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.size);
        if self.size > 0 {
            let first = self.size.min(self.limit - self.start);
            out.extend_from_slice(&self.buf[self.start..self.start + first]);
            let rem = self.size - first;
            if rem > 0 {
                out.extend_from_slice(&self.buf[..rem]);
            }
        }
        self.start = 0;
        self.size = 0;
        out
    }

    fn finish_text(&mut self) -> (String, bool) {
        let raw = self.take_bytes();
        let trimmed = if self.omitted {
            trim_utf8_boundary(&raw, self.tail)
        } else {
            raw.as_slice()
        };
        (to_valid_utf8_collapsed(trimmed), self.omitted)
    }
}

/// Bounded streaming line window (optionally also byte-bounded).
struct LineWindow {
    max_lines: u64,
    byte_limit: Option<usize>,
    tail: bool,
    rows: VecDeque<Vec<u8>>,
    row_bytes: usize,
    pending: Vec<u8>,
    pending_omitted: bool,
    omitted: bool,
}

impl LineWindow {
    fn new(max_lines: u64, byte_limit: Option<usize>, tail: bool) -> Self {
        Self {
            max_lines,
            byte_limit,
            tail,
            rows: VecDeque::new(),
            row_bytes: 0,
            pending: Vec::new(),
            pending_omitted: false,
            omitted: false,
        }
    }

    fn write(&mut self, mut data: &[u8]) {
        while !data.is_empty() {
            if !self.tail
                && (self.rows.len() as u64 >= self.max_lines
                    || self.byte_limit.is_some_and(|lim| self.row_bytes >= lim))
            {
                self.omitted = true;
                break;
            }
            if let Some(pos) = data.iter().position(|&b| b == b'\n') {
                self.write_pending(&data[..=pos]);
                self.finish_line();
                data = &data[pos + 1..];
            } else {
                self.write_pending(data);
                break;
            }
        }
    }

    fn write_pending(&mut self, chunk: &[u8]) {
        let Some(limit) = self.byte_limit else {
            self.pending.extend_from_slice(chunk);
            return;
        };
        if !self.tail {
            let space = limit.saturating_sub(self.pending.len());
            let take = chunk.len().min(space);
            self.pending.extend_from_slice(&chunk[..take]);
            if take < chunk.len() {
                self.pending_omitted = true;
            }
        } else {
            if chunk.len() >= limit {
                if !self.pending.is_empty() || chunk.len() > limit {
                    self.pending_omitted = true;
                }
                self.pending.clear();
                self.pending
                    .extend_from_slice(&chunk[chunk.len() - limit..]);
            } else {
                let total = self.pending.len() + chunk.len();
                if total > limit {
                    self.pending_omitted = true;
                    let drop_n = total - limit;
                    self.pending.drain(..drop_n);
                }
                self.pending.extend_from_slice(chunk);
            }
        }
    }

    fn finish_line(&mut self) {
        let line = std::mem::take(&mut self.pending);
        if self.pending_omitted {
            self.omitted = true;
            self.pending_omitted = false;
        }
        if !self.tail {
            if let Some(limit) = self.byte_limit {
                let space = limit.saturating_sub(self.row_bytes);
                let take = line.len().min(space);
                if take < line.len() {
                    self.omitted = true;
                }
                self.rows.push_back(line[..take].to_vec());
                self.row_bytes += take;
            } else {
                self.row_bytes += line.len();
                self.rows.push_back(line);
            }
            return;
        }

        self.row_bytes += line.len();
        self.rows.push_back(line);
        while (self.rows.len() as u64) > self.max_lines
            || self.byte_limit.is_some_and(|lim| self.row_bytes > lim)
        {
            self.omitted = true;
            if let Some(limit) = self.byte_limit
                && (self.rows.len() as u64) <= self.max_lines
                && let Some(front) = self.rows.front_mut()
            {
                let excess = self.row_bytes - limit;
                if excess < front.len() {
                    front.drain(..excess);
                    self.row_bytes -= excess;
                    break;
                }
            }
            if let Some(dropped) = self.rows.pop_front() {
                self.row_bytes -= dropped.len();
            }
        }
    }

    fn finish_text(mut self) -> (String, bool) {
        if !self.pending.is_empty() {
            self.finish_line();
        }
        let mut combined = Vec::with_capacity(self.row_bytes);
        for row in self.rows {
            combined.extend_from_slice(&row);
        }
        let trimmed = if self.omitted && self.byte_limit.is_some() {
            trim_utf8_boundary(&combined, self.tail)
        } else {
            combined.as_slice()
        };
        (to_valid_utf8_collapsed(trimmed), self.omitted)
    }
}

fn trim_utf8_boundary(bytes: &[u8], tail: bool) -> &[u8] {
    if tail {
        let mut start = 0;
        while start < bytes.len() && (bytes[start] & 0b1100_0000) == 0b1000_0000 {
            start += 1;
        }
        &bytes[start..]
    } else {
        let mut end = bytes.len();
        // Trim at most 3 trailing incomplete UTF-8 continuation/lead bytes at a cut boundary.
        while end > 0 {
            let last = bytes[end - 1];
            if last < 0x80 {
                break;
            }
            // Check if bytes[..end] ends with a valid UTF-8 character.
            let check_start = end.saturating_sub(4);
            let mut probe = check_start;
            while probe < end && (bytes[probe] & 0b1100_0000) == 0b1000_0000 {
                probe += 1;
            }
            if std::str::from_utf8(&bytes[probe..end]).is_ok() {
                break;
            }
            end -= 1;
        }
        &bytes[..end]
    }
}

enum StreamCapture {
    Bytes(ByteWindow),
    Lines(LineWindow),
}

impl StreamCapture {
    fn new(options: &RunOptions) -> Self {
        let byte_limit = options
            .tokens
            .map(|t| t.saturating_mul(4).saturating_add(4));
        if let Some(max_lines) = options.lines {
            Self::Lines(LineWindow::new(max_lines, byte_limit, options.tail))
        } else {
            Self::Bytes(ByteWindow::new(byte_limit.unwrap_or(0), options.tail))
        }
    }

    fn push_bytes(&mut self, data: &[u8]) {
        match self {
            Self::Bytes(w) => w.write(data),
            Self::Lines(w) => w.write(data),
        }
    }

    fn finish(self) -> (String, bool) {
        match self {
            Self::Bytes(mut w) => w.finish_text(),
            Self::Lines(w) => w.finish_text(),
        }
    }
}

fn approx_tokens(s: &str) -> usize {
    s.len().div_ceil(4)
}

fn select_token_slice(s: &str, token_budget: usize, tail: bool) -> &str {
    if token_budget == 0 {
        return "";
    }
    let max_bytes = token_budget.saturating_mul(4);
    if s.len() <= max_bytes {
        return s;
    }
    if tail {
        let start = s.ceil_char_boundary(s.len().saturating_sub(max_bytes));
        &s[start..]
    } else {
        let end = s.floor_char_boundary(max_bytes);
        &s[..end]
    }
}

fn exit_code_from_status(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    1
}

pub fn execute(args: &[String]) -> ToolOutput {
    let (options, argv) = match parse_options(args) {
        Ok(parsed) => parsed,
        Err(err) => {
            return ToolOutput {
                stdout: String::new(),
                stderr: format!("mrun: {err}\n"),
                exit_code: 2,
            };
        }
    };

    let resolved = match which::which(&argv[0]) {
        Ok(path) => path,
        Err(err) => {
            return ToolOutput {
                stdout: String::new(),
                stderr: format!("{}: {err}\n", argv[0]),
                exit_code: 127,
            };
        }
    };

    let mut cmd = Command::new(&resolved);
    cmd.args(&argv[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::common::configure_new_process_group(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            let code = if err.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                1
            };
            return ToolOutput {
                stdout: String::new(),
                stderr: format!("{}: {err}\n", argv[0]),
                exit_code: code,
            };
        }
    };

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let out_cap = std::sync::Arc::new(std::sync::Mutex::new(Some(StreamCapture::new(&options))));
    let err_cap = std::sync::Arc::new(std::sync::Mutex::new(Some(StreamCapture::new(&options))));

    let out_cap_thread = std::sync::Arc::clone(&out_cap);
    let (out_rx, out_handle) = crate::common::spawn_bounded_reader(move || {
        if let Some(mut pipe) = stdout_pipe {
            let mut buf = [0u8; 32 * 1024];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut guard) = out_cap_thread.lock()
                    && let Some(cap) = guard.as_mut()
                {
                    cap.push_bytes(&buf[..n]);
                }
            }
        }
    });
    let err_cap_thread = std::sync::Arc::clone(&err_cap);
    let (err_rx, err_handle) = crate::common::spawn_bounded_reader(move || {
        if let Some(mut pipe) = stderr_pipe {
            let mut buf = [0u8; 32 * 1024];
            while let Ok(n) = pipe.read(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(mut guard) = err_cap_thread.lock()
                    && let Some(cap) = guard.as_mut()
                {
                    cap.push_bytes(&buf[..n]);
                }
            }
        }
    });

    let status = match child.wait() {
        Ok(status) => status,
        Err(err) => {
            crate::common::retire_child_process_group(&mut child);
            return ToolOutput {
                stdout: String::new(),
                stderr: format!("mrun: wait child: {err}\n"),
                exit_code: 1,
            };
        }
    };

    // Terminate any descendants still holding inherited stdout/stderr pipes so draining cannot hang.
    crate::common::kill_process_group(child.id() as i32);

    let drain_timeout = std::time::Duration::from_secs(2);
    let _ = crate::common::recv_reader_with_timeout(out_rx, out_handle, drain_timeout);
    let _ = crate::common::recv_reader_with_timeout(err_rx, err_handle, drain_timeout);
    let (out_text, out_omitted) = out_cap
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
        .map(StreamCapture::finish)
        .unwrap_or_default();
    let (err_text, err_omitted) = err_cap
        .lock()
        .ok()
        .and_then(|mut guard| guard.take())
        .map(StreamCapture::finish)
        .unwrap_or_default();

    let (selected_out, selected_err) = if let Some(budget) = options.tokens {
        // Reserve at most half the token budget for stderr when stdout is non-empty.
        let err_budget = if !out_text.is_empty() {
            budget / 2
        } else {
            budget
        };
        let sel_err = select_token_slice(&err_text, err_budget, options.tail);
        let out_budget = budget.saturating_sub(approx_tokens(sel_err));
        let sel_out = select_token_slice(&out_text, out_budget, options.tail);
        (sel_out.to_string(), sel_err.to_string())
    } else {
        (out_text.clone(), err_text.clone())
    };

    let was_omitted =
        out_omitted || err_omitted || selected_out != out_text || selected_err != err_text;
    let mut final_stderr = selected_err;
    if was_omitted {
        let limit_desc = match (options.lines, options.tokens) {
            (Some(lines), Some(tokens)) => format!("{lines}-line limit or {tokens}-token limit"),
            (Some(lines), None) => format!("{lines}-line limit"),
            (None, Some(tokens)) => format!("{tokens}-token limit"),
            (None, None) => String::new(),
        };
        if !final_stderr.is_empty() && !final_stderr.ends_with('\n') {
            final_stderr.push('\n');
        }
        final_stderr.push_str(&format!("mrun: output incomplete: {limit_desc} reached\n"));
    }

    ToolOutput {
        stdout: selected_out,
        stderr: final_stderr,
        exit_code: exit_code_from_status(status),
    }
}

#[cfg(test)]
mod tests;
