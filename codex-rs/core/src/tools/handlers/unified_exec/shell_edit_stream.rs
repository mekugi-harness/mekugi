//! Incremental model-token-stream projector for authored shell and Python edits.
//!
//! Projects streaming `exec_command` argument deltas (`cat <<EOF > path`,
//! `tee path <<EOF`, and inline Python `open`/`read_text` -> `replace` ->
//! `write_text`/`write`) into `EventMsg::PatchApplyUpdated` so they share the
//! exact same Live Diff rendering pipeline as `apply_patch`.
//!
//! Non-streaming writes whose bytes come from disk rather than the model's
//! token stream (`cat < big > out`, `cp`, `shutil.copy*`, etc.) emit nothing.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use crate::function_tool::FunctionCallError;
use crate::session::turn_context::TurnContext;
use crate::tools::registry::ToolArgumentDiffConsumer;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::PatchApplyUpdatedEvent;

const SHELL_EDIT_STREAM_BUFFER_INTERVAL: Duration = Duration::from_millis(100);
const MAX_STREAM_FILES: usize = 32;
const MAX_STREAM_FILE_BYTES: usize = 128 * 1024;

#[derive(Default)]
pub(crate) struct ExecCommandArgumentDiffConsumer {
    json: JsonCmdExtractor,
    projector: Option<ShellEditProjector>,
    call_id: Option<String>,
    last_sent_at: Option<Instant>,
    pending: Option<PatchApplyUpdatedEvent>,
}

impl ToolArgumentDiffConsumer for ExecCommandArgumentDiffConsumer {
    fn consume_diff(
        &mut self,
        turn: &TurnContext,
        call_id: String,
        diff: &str,
    ) -> Option<EventMsg> {
        self.call_id = Some(call_id.clone());
        let projector = self.projector.get_or_insert_with(|| {
            #[allow(deprecated)]
            let cwd = turn.cwd.as_path();
            ShellEditProjector::new(cwd)
        });
        let (cmd_chunk, workdir) = self.json.push(diff);
        if let Some(dir) = workdir {
            projector.set_workdir(Path::new(&dir));
        }
        if !cmd_chunk.is_empty() {
            projector.push(&cmd_chunk);
        }
        self.maybe_emit(call_id)
    }

    fn finish(&mut self) -> Result<Option<EventMsg>, FunctionCallError> {
        if let Some(projector) = self.projector.as_mut() {
            projector.finish();
            if projector.take_dirty()
                && let Some(call_id) = self.call_id.clone()
            {
                let changes = projector.snapshot();
                if !changes.is_empty() {
                    self.pending = Some(PatchApplyUpdatedEvent { call_id, changes });
                }
            }
        }
        let event = self.pending.take();
        if event.is_some() {
            self.last_sent_at = Some(Instant::now());
        }
        Ok(event.map(EventMsg::PatchApplyUpdated))
    }
}

impl ExecCommandArgumentDiffConsumer {
    fn maybe_emit(&mut self, call_id: String) -> Option<EventMsg> {
        let projector = self.projector.as_mut()?;
        if !projector.take_dirty() {
            return None;
        }
        let changes = projector.snapshot();
        if changes.is_empty() {
            return None;
        }
        let event = PatchApplyUpdatedEvent { call_id, changes };
        let now = Instant::now();
        match self.last_sent_at {
            Some(last) if now.duration_since(last) < SHELL_EDIT_STREAM_BUFFER_INTERVAL => {
                self.pending = Some(event);
                None
            }
            _ => {
                self.pending = None;
                self.last_sent_at = Some(now);
                Some(EventMsg::PatchApplyUpdated(event))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Incremental JSON string field extractor for {"cmd": "...", "workdir": "..."}
// ---------------------------------------------------------------------------

#[derive(Default)]
struct JsonCmdExtractor {
    depth: usize,
    state: JsonState,
    key_buf: String,
    workdir_buf: String,
    escape: JsonEscape,
    high_surrogate: Option<u16>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum JsonState {
    #[default]
    Start,
    ExpectKey,
    InKey,
    AfterKey,
    ExpectValue(TargetField),
    InTargetString(TargetField),
    SkipValue,
    InSkipString,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TargetField {
    Cmd,
    Workdir,
    Other,
}

#[derive(Default, Clone, Copy)]
enum JsonEscape {
    #[default]
    None,
    Backslash,
    Unicode(u8, u16),
}

impl JsonCmdExtractor {
    fn push(&mut self, delta: &str) -> (String, Option<String>) {
        let mut cmd_out = String::new();
        let mut completed_workdir = None;
        for ch in delta.chars() {
            match self.state {
                JsonState::Start => {
                    if ch == '{' {
                        self.depth = 1;
                        self.state = JsonState::ExpectKey;
                    }
                }
                JsonState::ExpectKey => match ch {
                    '"' => {
                        self.key_buf.clear();
                        self.state = JsonState::InKey;
                    }
                    '}' => {
                        self.depth = self.depth.saturating_sub(1);
                        self.state = JsonState::Start;
                    }
                    _ => {}
                },
                JsonState::InKey => {
                    if let Some(decoded) = self.feed_string_char(ch) {
                        self.key_buf.push(decoded);
                    } else if matches!(self.escape, JsonEscape::None) && ch == '"' {
                        self.state = JsonState::AfterKey;
                    }
                }
                JsonState::AfterKey => {
                    if ch == ':' {
                        let field = match self.key_buf.as_str() {
                            "cmd" => TargetField::Cmd,
                            "workdir" => TargetField::Workdir,
                            _ => TargetField::Other,
                        };
                        self.state = JsonState::ExpectValue(field);
                    }
                }
                JsonState::ExpectValue(field) => {
                    if ch.is_whitespace() {
                        continue;
                    }
                    match (field, ch) {
                        (TargetField::Cmd | TargetField::Workdir, '"') => {
                            if field == TargetField::Workdir {
                                self.workdir_buf.clear();
                            }
                            self.state = JsonState::InTargetString(field);
                        }
                        (_, '"') => {
                            self.state = JsonState::InSkipString;
                        }
                        (_, '{' | '[') => {
                            self.depth += 1;
                            self.state = JsonState::SkipValue;
                        }
                        _ => {
                            self.state = JsonState::SkipValue;
                        }
                    }
                }
                JsonState::InTargetString(field) => {
                    if let Some(decoded) = self.feed_string_char(ch) {
                        match field {
                            TargetField::Cmd => cmd_out.push(decoded),
                            TargetField::Workdir => self.workdir_buf.push(decoded),
                            TargetField::Other => {}
                        }
                    } else if matches!(self.escape, JsonEscape::None) && ch == '"' {
                        if field == TargetField::Workdir && !self.workdir_buf.is_empty() {
                            completed_workdir = Some(std::mem::take(&mut self.workdir_buf));
                        }
                        self.state = JsonState::SkipValue;
                    }
                }
                JsonState::InSkipString => {
                    let _ = self.feed_string_char(ch);
                    if matches!(self.escape, JsonEscape::None) && ch == '"' {
                        self.state = JsonState::SkipValue;
                    }
                }
                JsonState::SkipValue => match ch {
                    '"' => self.state = JsonState::InSkipString,
                    '{' | '[' => self.depth += 1,
                    '}' | ']' => {
                        self.depth = self.depth.saturating_sub(1);
                        if self.depth == 0 {
                            self.state = JsonState::Start;
                        }
                    }
                    ',' if self.depth == 1 => {
                        self.state = JsonState::ExpectKey;
                    }
                    _ => {}
                },
            }
        }
        (cmd_out, completed_workdir)
    }

    /// Decodes one char inside a JSON string. Returns `None` on closing `"` or
    /// while accumulating an escape sequence.
    fn feed_string_char(&mut self, ch: char) -> Option<char> {
        match self.escape {
            JsonEscape::None => match ch {
                '\\' => {
                    self.escape = JsonEscape::Backslash;
                    None
                }
                '"' => {
                    self.high_surrogate = None;
                    None
                }
                other => {
                    self.high_surrogate = None;
                    Some(other)
                }
            },
            JsonEscape::Backslash => {
                self.escape = JsonEscape::None;
                match ch {
                    '"' => Some('"'),
                    '\\' => Some('\\'),
                    '/' => Some('/'),
                    'b' => Some('\u{0008}'),
                    'f' => Some('\u{000C}'),
                    'n' => Some('\n'),
                    'r' => Some('\r'),
                    't' => Some('\t'),
                    'u' => {
                        self.escape = JsonEscape::Unicode(0, 0);
                        None
                    }
                    other => Some(other),
                }
            }
            JsonEscape::Unicode(count, acc) => {
                let digit = ch.to_digit(16).unwrap_or(0) as u16;
                let next = (acc << 4) | digit;
                if count + 1 < 4 {
                    self.escape = JsonEscape::Unicode(count + 1, next);
                    None
                } else {
                    self.escape = JsonEscape::None;
                    if (0xD800..=0xDBFF).contains(&next) {
                        self.high_surrogate = Some(next);
                        None
                    } else if (0xDC00..=0xDFFF).contains(&next) {
                        if let Some(hi) = self.high_surrogate.take() {
                            let cp =
                                0x10000 + (((hi as u32 - 0xD800) << 10) | (next as u32 - 0xDC00));
                            char::from_u32(cp)
                        } else {
                            Some('\u{FFFD}')
                        }
                    } else {
                        self.high_surrogate = None;
                        char::from_u32(next as u32)
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shell edit stream projector (cat/tee heredocs + inline Python text edits)
// ---------------------------------------------------------------------------

pub(crate) struct ShellEditProjector {
    cwd: PathBuf,
    mode: ShellMode,
    files: HashMap<PathBuf, StreamedFile>,
    dirty: bool,
}

enum StreamedFile {
    Add { committed: String, partial: String },
    Update { committed: String, partial: String },
}

impl StreamedFile {
    fn to_file_change(&self) -> Option<FileChange> {
        match self {
            Self::Add { committed, partial } => {
                if committed.is_empty() && partial.is_empty() {
                    return None;
                }
                let mut content = String::with_capacity(committed.len() + partial.len());
                content.push_str(committed);
                content.push_str(partial);
                Some(FileChange::Add { content })
            }
            Self::Update { committed, partial } => {
                if committed.is_empty() && partial.is_empty() {
                    return None;
                }
                let mut unified_diff = String::with_capacity(committed.len() + partial.len());
                unified_diff.push_str(committed);
                unified_diff.push_str(partial);
                Some(FileChange::Update {
                    unified_diff,
                    move_path: None,
                })
            }
        }
    }
}

enum ShellMode {
    Scan {
        line: String,
    },
    Heredoc {
        path: PathBuf,
        delimiter: String,
        strip_tabs: bool,
        line_buf: String,
    },
    PythonHeredoc {
        delimiter: String,
        strip_tabs: bool,
        line_buf: String,
        py: PythonEditProjector,
    },
    PythonInline {
        quote: char,
        escaped: bool,
        py: PythonEditProjector,
    },
}

impl ShellEditProjector {
    pub(crate) fn new(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            mode: ShellMode::Scan {
                line: String::new(),
            },
            files: HashMap::new(),
            dirty: false,
        }
    }

    pub(crate) fn set_workdir(&mut self, dir: &Path) {
        if dir.is_absolute() {
            self.cwd = dir.to_path_buf();
        } else {
            self.cwd = self.cwd.join(dir);
        }
    }

    pub(crate) fn push(&mut self, chunk: &str) {
        for ch in chunk.chars() {
            self.push_char(ch);
        }
    }

    pub(crate) fn finish(&mut self) {
        match &mut self.mode {
            ShellMode::Scan { line } => {
                let pending = std::mem::take(line);
                if !pending.trim().is_empty() {
                    self.handle_scan_line(&pending);
                }
            }
            ShellMode::Heredoc {
                path,
                delimiter,
                strip_tabs,
                line_buf,
            } => {
                let tail = std::mem::take(line_buf);
                let trimmed = if *strip_tabs {
                    tail.trim_start_matches('\t')
                } else {
                    tail.as_str()
                };
                if !tail.is_empty() && trimmed != delimiter.as_str() {
                    let path = path.clone();
                    let content = trimmed.to_owned();
                    self.commit_add_line(&path, &content, false);
                }
            }
            ShellMode::PythonHeredoc {
                delimiter,
                strip_tabs,
                line_buf,
                py,
            } => {
                let tail = std::mem::take(line_buf);
                let trimmed = if *strip_tabs {
                    tail.trim_start_matches('\t')
                } else {
                    tail.as_str()
                };
                if !tail.is_empty() && trimmed != delimiter.as_str() {
                    py.push(trimmed, &mut self.files, &mut self.dirty);
                }
                py.finish(&mut self.files, &mut self.dirty);
            }
            ShellMode::PythonInline { py, .. } => {
                py.finish(&mut self.files, &mut self.dirty);
            }
        }
    }

    pub(crate) fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    pub(crate) fn snapshot(&self) -> HashMap<PathBuf, FileChange> {
        self.files
            .iter()
            .filter_map(|(path, file)| Some((path.clone(), file.to_file_change()?)))
            .collect()
    }

    fn push_char(&mut self, ch: char) {
        enum Transition {
            None,
            ToScan,
            ToPythonInline(char),
        }
        let mut transition = Transition::None;
        match &mut self.mode {
            ShellMode::Scan { line } => {
                if ch == '\n' {
                    let completed = std::mem::take(line);
                    self.handle_scan_line(&completed);
                    return;
                }
                line.push(ch);
                if (ch == '"' || ch == '\'')
                    && let Some(quote) = detect_python_inline_start(line)
                {
                    line.clear();
                    transition = Transition::ToPythonInline(quote);
                }
            }
            ShellMode::Heredoc {
                path,
                delimiter,
                strip_tabs,
                line_buf,
            } => {
                if ch == '\n' {
                    let raw_line = std::mem::take(line_buf);
                    let line = if *strip_tabs {
                        raw_line.trim_start_matches('\t')
                    } else {
                        raw_line.as_str()
                    };
                    let line = line.strip_suffix('\r').unwrap_or(line);
                    if line == delimiter.as_str() {
                        let path = path.clone();
                        self.set_add_partial(&path, "");
                        transition = Transition::ToScan;
                    } else {
                        let path = path.clone();
                        let owned = line.to_owned();
                        self.commit_add_line(&path, &owned, true);
                    }
                } else {
                    line_buf.push(ch);
                    let candidate = if *strip_tabs {
                        line_buf.trim_start_matches('\t')
                    } else {
                        line_buf.as_str()
                    };
                    let path = path.clone();
                    // Avoid flashing the closing delimiter as file content.
                    if !delimiter.starts_with(candidate) {
                        let owned = candidate.to_owned();
                        self.set_add_partial(&path, &owned);
                    }
                }
            }
            ShellMode::PythonHeredoc {
                delimiter,
                strip_tabs,
                line_buf,
                py,
            } => {
                if ch == '\n' {
                    let raw_line = std::mem::take(line_buf);
                    let line = if *strip_tabs {
                        raw_line.trim_start_matches('\t')
                    } else {
                        raw_line.as_str()
                    };
                    let line = line.strip_suffix('\r').unwrap_or(line);
                    if line == delimiter.as_str() {
                        py.finish(&mut self.files, &mut self.dirty);
                        transition = Transition::ToScan;
                    } else {
                        py.push(line, &mut self.files, &mut self.dirty);
                        py.push("\n", &mut self.files, &mut self.dirty);
                    }
                } else {
                    line_buf.push(ch);
                }
            }
            ShellMode::PythonInline { quote, escaped, py } => {
                if *quote == '"' {
                    if *escaped {
                        *escaped = false;
                        let decoded = match ch {
                            '"' => '"',
                            '\\' => '\\',
                            '$' => '$',
                            '`' => '`',
                            other => {
                                py.push("\\", &mut self.files, &mut self.dirty);
                                other
                            }
                        };
                        let mut buf = [0u8; 4];
                        py.push(
                            decoded.encode_utf8(&mut buf),
                            &mut self.files,
                            &mut self.dirty,
                        );
                    } else if ch == '\\' {
                        *escaped = true;
                    } else if ch == '"' {
                        py.finish(&mut self.files, &mut self.dirty);
                        transition = Transition::ToScan;
                    } else {
                        let mut buf = [0u8; 4];
                        py.push(ch.encode_utf8(&mut buf), &mut self.files, &mut self.dirty);
                    }
                } else if ch == '\'' {
                    py.finish(&mut self.files, &mut self.dirty);
                    transition = Transition::ToScan;
                } else {
                    let mut buf = [0u8; 4];
                    py.push(ch.encode_utf8(&mut buf), &mut self.files, &mut self.dirty);
                }
            }
        }
        match transition {
            Transition::None => {}
            Transition::ToScan => {
                self.mode = ShellMode::Scan {
                    line: String::new(),
                };
            }
            Transition::ToPythonInline(quote) => {
                self.mode = ShellMode::PythonInline {
                    quote,
                    escaped: false,
                    py: PythonEditProjector::new(self.cwd.clone()),
                };
            }
        }
    }

    fn handle_scan_line(&mut self, line: &str) {
        for segment in split_shell_chain(line) {
            let tokens = tokenize_shell_segment(segment);
            if tokens.is_empty() {
                continue;
            }
            if tokens[0] == "cd" && tokens.len() == 2 && !tokens[1].starts_with('-') {
                if is_literal_path(&tokens[1]) {
                    self.set_workdir(Path::new(&tokens[1]));
                }
                continue;
            }
            if let Some(heredoc) = parse_heredoc_header(&tokens) {
                match heredoc {
                    HeredocHeader::FileWrite {
                        target,
                        delimiter,
                        strip_tabs,
                    } => {
                        let path = resolve_path(&self.cwd, &target);
                        self.mode = ShellMode::Heredoc {
                            path,
                            delimiter,
                            strip_tabs,
                            line_buf: String::new(),
                        };
                        return;
                    }
                    HeredocHeader::Python {
                        delimiter,
                        strip_tabs,
                    } => {
                        self.mode = ShellMode::PythonHeredoc {
                            delimiter,
                            strip_tabs,
                            line_buf: String::new(),
                            py: PythonEditProjector::new(self.cwd.clone()),
                        };
                        return;
                    }
                }
            }
        }
    }

    fn commit_add_line(&mut self, path: &Path, line: &str, trailing_newline: bool) {
        if !self.files.contains_key(path) && self.files.len() >= MAX_STREAM_FILES {
            return;
        }
        let entry = self
            .files
            .entry(path.to_path_buf())
            .or_insert_with(|| StreamedFile::Add {
                committed: String::new(),
                partial: String::new(),
            });
        if let StreamedFile::Add { committed, partial } = entry {
            partial.clear();
            if committed.len() + line.len() + usize::from(trailing_newline) <= MAX_STREAM_FILE_BYTES
            {
                committed.push_str(line);
                if trailing_newline {
                    committed.push('\n');
                }
                self.dirty = true;
            }
        }
    }

    fn set_add_partial(&mut self, path: &Path, text: &str) {
        if !self.files.contains_key(path) && self.files.len() >= MAX_STREAM_FILES {
            return;
        }
        let entry = self
            .files
            .entry(path.to_path_buf())
            .or_insert_with(|| StreamedFile::Add {
                committed: String::new(),
                partial: String::new(),
            });
        if let StreamedFile::Add { committed, partial } = entry {
            if committed.len() + text.len() <= MAX_STREAM_FILE_BYTES && partial != text {
                partial.clear();
                partial.push_str(text);
                self.dirty = true;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Shell header helpers (heredoc + python -c detection)
// ---------------------------------------------------------------------------

enum HeredocHeader {
    FileWrite {
        target: String,
        delimiter: String,
        strip_tabs: bool,
    },
    Python {
        delimiter: String,
        strip_tabs: bool,
    },
}

fn detect_python_inline_start(line: &str) -> Option<char> {
    let quote = line.chars().last()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let prefix = &line[..line.len() - quote.len_utf8()];
    let last_segment = split_shell_chain(prefix).last().copied().unwrap_or(prefix);
    let tokens = tokenize_shell_segment(last_segment);
    if tokens.len() == 2 && is_python_bin(&tokens[0]) && tokens[1] == "-c" {
        Some(quote)
    } else {
        None
    }
}

fn is_python_bin(token: &str) -> bool {
    let name = Path::new(token)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(token);
    matches!(name, "python" | "python3")
}

fn parse_heredoc_header(tokens: &[String]) -> Option<HeredocHeader> {
    let cmd = Path::new(&tokens[0])
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&tokens[0]);
    let mut delimiter: Option<(String, bool)> = None;
    let mut output_target: Option<String> = None;
    let mut positional: Vec<&str> = Vec::new();

    let mut i = 1;
    while i < tokens.len() {
        let tok = tokens[i].as_str();
        if tok == "<" || tok == "0<" || tok.starts_with('<') && !tok.starts_with("<<") {
            // File input redirect (e.g. `cat < big.txt > out.txt`) is never streaming.
            return None;
        }
        if let Some(rest) = tok.strip_prefix("<<") {
            let (strip_tabs, raw_delim) = match rest.strip_prefix('-') {
                Some(after) => (true, after),
                None => (false, rest),
            };
            let delim = if raw_delim.is_empty() {
                i += 1;
                tokens.get(i)?.as_str()
            } else {
                raw_delim
            };
            let clean = strip_shell_quotes(delim);
            if clean.is_empty() || delimiter.is_some() {
                return None;
            }
            delimiter = Some((clean, strip_tabs));
        } else if matches!(tok, ">" | ">>" | ">|" | "1>" | "1>>") {
            i += 1;
            let target = tokens.get(i)?;
            if !is_literal_path(target) || output_target.is_some() {
                return None;
            }
            output_target = Some(strip_shell_quotes(target));
        } else if let Some(rest) = tok
            .strip_prefix("1>>")
            .or_else(|| tok.strip_prefix("1>"))
            .or_else(|| tok.strip_prefix(">>"))
            .or_else(|| tok.strip_prefix(">|"))
            .or_else(|| tok.strip_prefix('>'))
            && !rest.is_empty()
        {
            if !is_literal_path(rest) || output_target.is_some() {
                return None;
            }
            output_target = Some(strip_shell_quotes(rest));
        } else {
            positional.push(tok);
        }
        i += 1;
    }

    let (delimiter, strip_tabs) = delimiter?;
    match cmd {
        "cat" => {
            if !positional.is_empty() {
                return None;
            }
            let target = output_target?;
            Some(HeredocHeader::FileWrite {
                target,
                delimiter,
                strip_tabs,
            })
        }
        "tee" => {
            if output_target.is_some() {
                return None;
            }
            let files: Vec<&str> = positional
                .into_iter()
                .filter(|arg| *arg != "-a" && *arg != "--append")
                .collect();
            if files.len() != 1 || !is_literal_path(files[0]) {
                return None;
            }
            Some(HeredocHeader::FileWrite {
                target: strip_shell_quotes(files[0]),
                delimiter,
                strip_tabs,
            })
        }
        _ if is_python_bin(cmd) => {
            if output_target.is_some() {
                return None;
            }
            if positional.is_empty() || positional == ["-"] {
                Some(HeredocHeader::Python {
                    delimiter,
                    strip_tabs,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

fn split_shell_chain(line: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if in_single {
            if b == b'\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_double = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\\' => escaped = true,
            b'\'' => in_single = true,
            b'"' => in_double = true,
            b';' => {
                segments.push(&line[start..i]);
                start = i + 1;
            }
            b'&' if i + 1 < bytes.len() && bytes[i + 1] == b'&' => {
                segments.push(&line[start..i]);
                i += 1;
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    segments.push(&line[start..]);
    segments
}

fn tokenize_shell_segment(segment: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in segment.chars() {
        if escaped {
            escaped = false;
            current.push(ch);
            continue;
        }
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            current.push(ch);
            continue;
        }
        if in_double {
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
            current.push(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            match ch {
                '\\' => escaped = true,
                '\'' => {
                    in_single = true;
                    current.push(ch);
                }
                '"' => {
                    in_double = true;
                    current.push(ch);
                }
                _ => current.push(ch),
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn strip_shell_quotes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_single = false;
    let mut in_double = false;
    for ch in s.chars() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            other => out.push(other),
        }
    }
    out
}

fn is_literal_path(raw: &str) -> bool {
    if raw.is_empty() {
        return false;
    }
    let clean = strip_shell_quotes(raw);
    if clean.is_empty() {
        return false;
    }
    // Reject shell expansions or globs unless single-quoted.
    if !raw.starts_with('\'') && clean.contains(['$', '`', '*', '?', '[']) {
        return false;
    }
    true
}

fn resolve_path(cwd: &Path, target: &str) -> PathBuf {
    let path = Path::new(target);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

// ---------------------------------------------------------------------------
// Incremental Python edit projector (open/read_text -> replace -> write_text)
// ---------------------------------------------------------------------------

struct PythonEditProjector {
    cwd: PathBuf,
    /// `var -> path` from `p = Path("...")` or `p = "..."`
    path_vars: HashMap<String, PathBuf>,
    /// `var -> path` from `with open("...", mode) as f:` or `f = open(...)`
    file_vars: HashMap<String, PathBuf>,
    /// `var -> path` from `s = p.read_text()` or `s = f.read()`
    text_vars: HashMap<String, PathBuf>,
    /// Recent tokens on the current statement (reset on newline/semicolon at paren depth 0).
    stmt: Vec<PyTok>,
    paren_depth: usize,
    lex: PyLexState,
    active_op: Option<PyStreamOp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PyTok {
    Ident(String),
    Str(String),
    Dot,
    LParen,
    RParen,
    Comma,
    Eq,
    Colon,
    Other,
}

enum PyLexState {
    Normal {
        ident: String,
    },
    Comment,
    /// Inside a string literal.
    String {
        quote: char,
        triple: bool,
        raw: bool,
        /// Count of consecutive quotes just seen while checking for triple-quote start or end.
        pending_quotes: u8,
        /// True right after seeing the first quote, waiting to see if it's `"""` or `""`.
        checking_open: bool,
        escaped: bool,
        buf: String,
        streaming: bool,
    },
}

enum PyStreamOp {
    /// Streaming literal string argument to `write_text("...")` or `write("...")`.
    WriteLiteral { path: PathBuf },
    /// Waiting for the opening string of `replace(<old>, <new>)`.
    ReplaceAwaitOld { path: PathBuf },
    /// Streaming `<old>` string of `replace(<old>, <new>)`.
    ReplaceOld { path: PathBuf, old: String },
    /// Waiting for `,` then `<new>` string of `replace(<old>, <new>)`.
    ReplaceAwaitNew { path: PathBuf, old: String },
    /// Streaming `<new>` string of `replace(<old>, <new>)`.
    ReplaceNew {
        path: PathBuf,
        old: String,
        new: String,
    },
}

impl PythonEditProjector {
    fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            path_vars: HashMap::new(),
            file_vars: HashMap::new(),
            text_vars: HashMap::new(),
            stmt: Vec::new(),
            paren_depth: 0,
            lex: PyLexState::Normal {
                ident: String::new(),
            },
            active_op: None,
        }
    }

    fn push(&mut self, chunk: &str, files: &mut HashMap<PathBuf, StreamedFile>, dirty: &mut bool) {
        for ch in chunk.chars() {
            self.push_char(ch, files, dirty);
        }
    }

    fn finish(&mut self, files: &mut HashMap<PathBuf, StreamedFile>, dirty: &mut bool) {
        self.flush_ident();
        self.eval_statement();
        // Commit any in-progress partial hunk or literal write.
        if let Some(op) = self.active_op.take() {
            match op {
                PyStreamOp::WriteLiteral { path } => {
                    commit_partial(files, &path, dirty);
                }
                PyStreamOp::ReplaceOld { path, old }
                | PyStreamOp::ReplaceAwaitNew { path, old } => {
                    commit_replace_hunk(files, &path, &old, "", dirty);
                }
                PyStreamOp::ReplaceNew { path, old, new } => {
                    commit_replace_hunk(files, &path, &old, &new, dirty);
                }
                PyStreamOp::ReplaceAwaitOld { .. } => {}
            }
        }
    }

    fn push_char(
        &mut self,
        ch: char,
        files: &mut HashMap<PathBuf, StreamedFile>,
        dirty: &mut bool,
    ) {
        let mut replay_char: Option<char> = None;
        match &mut self.lex {
            PyLexState::Comment => {
                if ch == '\n' {
                    self.lex = PyLexState::Normal {
                        ident: String::new(),
                    };
                    if self.paren_depth == 0 {
                        self.eval_statement();
                    }
                }
                return;
            }
            PyLexState::String {
                quote,
                triple,
                raw: _,
                pending_quotes,
                checking_open,
                escaped: _,
                buf,
                streaming,
            } => {
                let q = *quote;
                let is_streaming = *streaming;
                if *checking_open {
                    if ch == q {
                        *pending_quotes += 1;
                        if *pending_quotes == 3 {
                            *triple = true;
                            *checking_open = false;
                            *pending_quotes = 0;
                        }
                        return;
                    } else if *pending_quotes == 2 {
                        // Empty string `""` or `''` followed by `ch`.
                        let finished = std::mem::take(buf);
                        self.lex = PyLexState::Normal {
                            ident: String::new(),
                        };
                        self.on_string_end(finished, is_streaming, files, dirty);
                        replay_char = Some(ch);
                    } else {
                        *checking_open = false;
                        *pending_quotes = 0;
                        // Fall through to process `ch` as first char of single-quoted string.
                    }
                }
                if replay_char.is_none()
                    && let PyLexState::String {
                        quote,
                        triple,
                        raw,
                        pending_quotes,
                        escaped,
                        buf,
                        streaming,
                        ..
                    } = &mut self.lex
                {
                    let q = *quote;
                    let is_streaming = *streaming;
                    if *triple {
                        if *escaped {
                            *escaped = false;
                            let decoded = decode_py_escape(ch, *raw);
                            push_py_str_piece(
                                buf,
                                decoded,
                                is_streaming,
                                &mut self.active_op,
                                files,
                                dirty,
                            );
                            return;
                        }
                        if ch == q {
                            *pending_quotes += 1;
                            if *pending_quotes == 3 {
                                let finished = std::mem::take(buf);
                                self.lex = PyLexState::Normal {
                                    ident: String::new(),
                                };
                                self.on_string_end(finished, is_streaming, files, dirty);
                            }
                            return;
                        }
                        if *pending_quotes > 0 {
                            let qs: String =
                                std::iter::repeat_n(q, *pending_quotes as usize).collect();
                            *pending_quotes = 0;
                            push_py_str_slice(
                                buf,
                                &qs,
                                is_streaming,
                                &mut self.active_op,
                                files,
                                dirty,
                            );
                        }
                        if ch == '\\' && !*raw {
                            *escaped = true;
                            return;
                        }
                        let mut tmp = [0u8; 4];
                        push_py_str_slice(
                            buf,
                            ch.encode_utf8(&mut tmp),
                            is_streaming,
                            &mut self.active_op,
                            files,
                            dirty,
                        );
                        return;
                    } else {
                        if *escaped {
                            *escaped = false;
                            let decoded = decode_py_escape(ch, *raw);
                            push_py_str_piece(
                                buf,
                                decoded,
                                is_streaming,
                                &mut self.active_op,
                                files,
                                dirty,
                            );
                            return;
                        }
                        if ch == '\\' && !*raw {
                            *escaped = true;
                            return;
                        }
                        if ch == q {
                            let finished = std::mem::take(buf);
                            self.lex = PyLexState::Normal {
                                ident: String::new(),
                            };
                            self.on_string_end(finished, is_streaming, files, dirty);
                            return;
                        }
                        let mut tmp = [0u8; 4];
                        push_py_str_slice(
                            buf,
                            ch.encode_utf8(&mut tmp),
                            is_streaming,
                            &mut self.active_op,
                            files,
                            dirty,
                        );
                        return;
                    }
                }
            }
            PyLexState::Normal { .. } => {}
        }

        let ch = replay_char.unwrap_or(ch);
        // Normal lexer state
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if let PyLexState::Normal { ident } = &mut self.lex {
                ident.push(ch);
            }
            return;
        }

        if ch == '"' || ch == '\'' {
            let prefix = if let PyLexState::Normal { ident } = &mut self.lex {
                std::mem::take(ident)
            } else {
                String::new()
            };
            let lower = prefix.to_ascii_lowercase();
            let is_valid_prefix =
                lower.is_empty() || matches!(lower.as_str(), "r" | "u" | "b" | "rb" | "br");
            if !is_valid_prefix && !prefix.is_empty() {
                // e.g. f-string `f"..."` -> not a literal string, cancel any active op
                self.stmt.push(PyTok::Ident(prefix));
                self.cancel_pending_op();
            }
            let raw = lower.contains('r');
            let streaming = is_valid_prefix && self.begin_streaming_string();
            self.lex = PyLexState::String {
                quote: ch,
                triple: false,
                raw,
                pending_quotes: 1,
                checking_open: true,
                escaped: false,
                buf: String::new(),
                streaming,
            };
            return;
        }

        self.flush_ident();

        match ch {
            '#' => {
                self.lex = PyLexState::Comment;
            }
            '(' => {
                self.paren_depth += 1;
                self.stmt.push(PyTok::LParen);
                self.check_call_open();
            }
            ')' => {
                self.paren_depth = self.paren_depth.saturating_sub(1);
                self.stmt.push(PyTok::RParen);
                if matches!(
                    self.active_op,
                    Some(PyStreamOp::WriteLiteral { .. } | PyStreamOp::ReplaceAwaitOld { .. })
                ) {
                    self.active_op = None;
                }
            }
            '.' => self.stmt.push(PyTok::Dot),
            ',' => self.stmt.push(PyTok::Comma),
            '=' => self.stmt.push(PyTok::Eq),
            ':' => {
                self.stmt.push(PyTok::Colon);
                if self.paren_depth == 0 {
                    self.eval_statement();
                }
            }
            '\n' | ';' => {
                if self.paren_depth == 0 {
                    self.eval_statement();
                }
            }
            c if c.is_whitespace() => {}
            _ => {
                self.cancel_pending_op();
                self.stmt.push(PyTok::Other);
            }
        }
    }

    fn flush_ident(&mut self) {
        if let PyLexState::Normal { ident } = &mut self.lex
            && !ident.is_empty()
        {
            let name = std::mem::take(ident);
            // If we were waiting for a literal string right after `write_text(` or `replace(`,
            // seeing an identifier means it's a variable/call (e.g. `write_text(Path("a").read_text())`),
            // NOT a literal string edit!
            self.cancel_pending_op();
            self.stmt.push(PyTok::Ident(name));
        }
    }

    fn cancel_pending_op(&mut self) {
        if matches!(
            self.active_op,
            Some(
                PyStreamOp::WriteLiteral { .. }
                    | PyStreamOp::ReplaceAwaitOld { .. }
                    | PyStreamOp::ReplaceAwaitNew { .. }
            )
        ) {
            self.active_op = None;
        }
    }

    /// Called right after `(` is pushed to `self.stmt`.
    fn check_call_open(&mut self) {
        let n = self.stmt.len();
        if n < 3 {
            return;
        }
        // Look for `... . <method> (`
        let PyTok::Ident(method) = &self.stmt[n - 2] else {
            return;
        };
        if self.stmt[n - 3] != PyTok::Dot {
            return;
        }
        let receiver_tokens = &self.stmt[..n - 3];
        match method.as_str() {
            "write_text" => {
                if let Some(path) = self.resolve_path_expr(receiver_tokens) {
                    self.active_op = Some(PyStreamOp::WriteLiteral { path });
                }
            }
            "write" => {
                if let Some(path) = self.resolve_file_expr(receiver_tokens) {
                    self.active_op = Some(PyStreamOp::WriteLiteral { path });
                }
            }
            "replace" => {
                if let Some(path) = self.resolve_text_expr(receiver_tokens) {
                    self.active_op = Some(PyStreamOp::ReplaceAwaitOld { path });
                }
            }
            _ => {}
        }
    }

    fn begin_streaming_string(&mut self) -> bool {
        match self.active_op.take() {
            Some(PyStreamOp::WriteLiteral { path }) => {
                self.active_op = Some(PyStreamOp::WriteLiteral { path });
                true
            }
            Some(PyStreamOp::ReplaceAwaitOld { path }) => {
                self.active_op = Some(PyStreamOp::ReplaceOld {
                    path,
                    old: String::new(),
                });
                true
            }
            Some(PyStreamOp::ReplaceAwaitNew { path, old }) => {
                self.active_op = Some(PyStreamOp::ReplaceNew {
                    path,
                    old,
                    new: String::new(),
                });
                true
            }
            other => {
                self.active_op = other;
                false
            }
        }
    }

    fn on_string_end(
        &mut self,
        value: String,
        was_streaming: bool,
        files: &mut HashMap<PathBuf, StreamedFile>,
        dirty: &mut bool,
    ) {
        if was_streaming {
            match self.active_op.take() {
                Some(PyStreamOp::WriteLiteral { path }) => {
                    commit_partial(files, &path, dirty);
                }
                Some(PyStreamOp::ReplaceOld { path, old }) => {
                    self.active_op = Some(PyStreamOp::ReplaceAwaitNew { path, old });
                }
                Some(PyStreamOp::ReplaceNew { path, old, new }) => {
                    commit_replace_hunk(files, &path, &old, &new, dirty);
                }
                other => {
                    self.active_op = other;
                }
            }
        }
        self.stmt.push(PyTok::Str(value));
    }

    fn resolve_path_expr(&self, tokens: &[PyTok]) -> Option<PathBuf> {
        match tokens.last()? {
            PyTok::Ident(name) => self.path_vars.get(name).cloned(),
            PyTok::RParen => extract_path_call(tokens, &self.cwd),
            _ => None,
        }
    }

    fn resolve_file_expr(&self, tokens: &[PyTok]) -> Option<PathBuf> {
        match tokens.last()? {
            PyTok::Ident(name) => self.file_vars.get(name).cloned(),
            PyTok::RParen => extract_open_call(tokens, &self.cwd, &self.path_vars),
            _ => None,
        }
    }

    fn resolve_text_expr(&self, tokens: &[PyTok]) -> Option<PathBuf> {
        match tokens.last()? {
            PyTok::Ident(name) => self.text_vars.get(name).cloned(),
            PyTok::RParen => {
                // Chained calls like `Path("foo").read_text().replace(...)` or `s.replace(...).replace(...)`
                if let Some(p) = extract_read_text_chain(
                    tokens,
                    &self.cwd,
                    &self.path_vars,
                    &self.file_vars,
                    &self.text_vars,
                ) {
                    return Some(p);
                }
                None
            }
            _ => None,
        }
    }

    /// Evaluates a completed Python statement for simple variable bindings:
    /// - `p = Path("foo.py")` / `p = pathlib.Path("foo.py")` / `p = "foo.py"`
    /// - `with open("foo.py", ...) as f:` / `f = open("foo.py", ...)`
    /// - `s = p.read_text()` / `s = f.read()` / `s = s.replace(...)`
    fn eval_statement(&mut self) {
        let stmt = std::mem::take(&mut self.stmt);
        if stmt.is_empty() {
            return;
        }
        // `with open(...) as f:`
        if matches!(stmt.first(), Some(PyTok::Ident(k)) if k == "with")
            && let Some(as_pos) = stmt
                .iter()
                .position(|t| matches!(t, PyTok::Ident(k) if k == "as"))
            && let Some(PyTok::Ident(var)) = stmt.get(as_pos + 1)
            && let Some(path) = extract_open_call(&stmt[1..as_pos], &self.cwd, &self.path_vars)
        {
            self.file_vars.insert(var.clone(), path);
            return;
        }

        // `<var> = <rhs>`
        if let [PyTok::Ident(var), PyTok::Eq, rhs @ ..] = stmt.as_slice() {
            if let [PyTok::Str(lit)] = rhs {
                self.path_vars
                    .insert(var.clone(), resolve_path(&self.cwd, lit));
            } else if let Some(path) = extract_path_call(rhs, &self.cwd) {
                self.path_vars.insert(var.clone(), path);
            } else if let Some(path) = extract_open_call(rhs, &self.cwd, &self.path_vars) {
                self.file_vars.insert(var.clone(), path);
            } else if let Some(path) = extract_read_text_chain(
                rhs,
                &self.cwd,
                &self.path_vars,
                &self.file_vars,
                &self.text_vars,
            ) {
                self.text_vars.insert(var.clone(), path);
            }
        }
    }
}

fn extract_path_call(tokens: &[PyTok], cwd: &Path) -> Option<PathBuf> {
    // Matches `Path("foo")` or `pathlib.Path("foo")` at the end of `tokens`.
    let n = tokens.len();
    if n >= 4
        && matches!(&tokens[n - 4], PyTok::Ident(name) if name == "Path")
        && tokens[n - 3] == PyTok::LParen
        && let PyTok::Str(path) = &tokens[n - 2]
        && tokens[n - 1] == PyTok::RParen
    {
        return Some(resolve_path(cwd, path));
    }
    None
}

fn extract_open_call(
    tokens: &[PyTok],
    cwd: &Path,
    path_vars: &HashMap<String, PathBuf>,
) -> Option<PathBuf> {
    // Matches `open("foo", ...)` or `open(p, ...)`
    let open_pos = tokens
        .iter()
        .rposition(|t| matches!(t, PyTok::Ident(name) if name == "open"))?;
    if tokens.get(open_pos + 1) != Some(&PyTok::LParen) {
        return None;
    }
    match tokens.get(open_pos + 2)? {
        PyTok::Str(path) => Some(resolve_path(cwd, path)),
        PyTok::Ident(var) => path_vars.get(var).cloned(),
        _ => None,
    }
}

fn extract_read_text_chain(
    tokens: &[PyTok],
    cwd: &Path,
    path_vars: &HashMap<String, PathBuf>,
    file_vars: &HashMap<String, PathBuf>,
    text_vars: &HashMap<String, PathBuf>,
) -> Option<PathBuf> {
    // Look for `<receiver>.read_text(...)`, `<receiver>.read(...)`, or `<receiver>.replace(...)`.
    for (idx, tok) in tokens.iter().enumerate() {
        if let PyTok::Ident(method) = tok
            && idx >= 2
            && tokens[idx - 1] == PyTok::Dot
        {
            let recv = &tokens[..idx - 1];
            match method.as_str() {
                "read_text" => {
                    if let Some(PyTok::Ident(var)) = recv.last()
                        && let Some(p) = path_vars.get(var)
                    {
                        return Some(p.clone());
                    }
                    if let Some(p) = extract_path_call(recv, cwd) {
                        return Some(p);
                    }
                }
                "read" => {
                    if let Some(PyTok::Ident(var)) = recv.last()
                        && let Some(p) = file_vars.get(var)
                    {
                        return Some(p.clone());
                    }
                    if let Some(p) = extract_open_call(recv, cwd, path_vars) {
                        return Some(p);
                    }
                }
                "replace" => {
                    if let Some(PyTok::Ident(var)) = recv.last()
                        && let Some(p) = text_vars.get(var)
                    {
                        return Some(p.clone());
                    }
                }
                _ => {}
            }
        }
    }
    None
}

fn decode_py_escape(ch: char, raw: bool) -> &'static str {
    if raw {
        return "";
    }
    match ch {
        'n' => "\n",
        'r' => "\r",
        't' => "\t",
        '\\' => "\\",
        '\'' => "'",
        '"' => "\"",
        _ => "",
    }
}

fn push_py_str_piece(
    buf: &mut String,
    piece: &str,
    is_streaming: bool,
    active_op: &mut Option<PyStreamOp>,
    files: &mut HashMap<PathBuf, StreamedFile>,
    dirty: &mut bool,
) {
    if !piece.is_empty() {
        push_py_str_slice(buf, piece, is_streaming, active_op, files, dirty);
    }
}

fn push_py_str_slice(
    buf: &mut String,
    slice: &str,
    is_streaming: bool,
    active_op: &mut Option<PyStreamOp>,
    files: &mut HashMap<PathBuf, StreamedFile>,
    dirty: &mut bool,
) {
    if buf.len() + slice.len() <= MAX_STREAM_FILE_BYTES {
        buf.push_str(slice);
    }
    if !is_streaming {
        return;
    }
    match active_op {
        Some(PyStreamOp::WriteLiteral { path }) => {
            if !files.contains_key(path) && files.len() >= MAX_STREAM_FILES {
                return;
            }
            let entry = files
                .entry(path.clone())
                .or_insert_with(|| StreamedFile::Add {
                    committed: String::new(),
                    partial: String::new(),
                });
            if let StreamedFile::Add { committed, partial } = entry
                && committed.len() + partial.len() + slice.len() <= MAX_STREAM_FILE_BYTES
            {
                partial.push_str(slice);
                *dirty = true;
            }
        }
        Some(PyStreamOp::ReplaceOld { path, old }) => {
            if old.len() + slice.len() <= MAX_STREAM_FILE_BYTES {
                old.push_str(slice);
                update_replace_partial(files, path, old, "", dirty);
            }
        }
        Some(PyStreamOp::ReplaceNew { path, old, new }) => {
            if new.len() + slice.len() <= MAX_STREAM_FILE_BYTES {
                new.push_str(slice);
                update_replace_partial(files, path, old, new, dirty);
            }
        }
        _ => {}
    }
}

fn format_replace_hunk(old: &str, new: &str) -> String {
    let mut out = String::from("@@\n");
    if !old.is_empty() {
        for line in old.strip_suffix('\n').unwrap_or(old).split('\n') {
            out.push('-');
            out.push_str(line);
            out.push('\n');
        }
    }
    if !new.is_empty() {
        for line in new.strip_suffix('\n').unwrap_or(new).split('\n') {
            out.push('+');
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn update_replace_partial(
    files: &mut HashMap<PathBuf, StreamedFile>,
    path: &Path,
    old: &str,
    new: &str,
    dirty: &mut bool,
) {
    if !files.contains_key(path) && files.len() >= MAX_STREAM_FILES {
        return;
    }
    let entry = files
        .entry(path.to_path_buf())
        .or_insert_with(|| StreamedFile::Update {
            committed: String::new(),
            partial: String::new(),
        });
    if let StreamedFile::Update { committed, partial } = entry {
        let hunk = format_replace_hunk(old, new);
        if committed.len() + hunk.len() <= MAX_STREAM_FILE_BYTES && *partial != hunk {
            *partial = hunk;
            *dirty = true;
        }
    }
}

fn commit_replace_hunk(
    files: &mut HashMap<PathBuf, StreamedFile>,
    path: &Path,
    old: &str,
    new: &str,
    dirty: &mut bool,
) {
    if !files.contains_key(path) && files.len() >= MAX_STREAM_FILES {
        return;
    }
    let entry = files
        .entry(path.to_path_buf())
        .or_insert_with(|| StreamedFile::Update {
            committed: String::new(),
            partial: String::new(),
        });
    if let StreamedFile::Update { committed, partial } = entry {
        partial.clear();
        let hunk = format_replace_hunk(old, new);
        if committed.len() + hunk.len() <= MAX_STREAM_FILE_BYTES {
            committed.push_str(&hunk);
            *dirty = true;
        }
    }
}

fn commit_partial(files: &mut HashMap<PathBuf, StreamedFile>, path: &Path, dirty: &mut bool) {
    if let Some(StreamedFile::Add { committed, partial }) = files.get_mut(path)
        && !partial.is_empty()
    {
        committed.push_str(&std::mem::take(partial));
        *dirty = true;
    }
}

#[cfg(test)]
#[path = "shell_edit_stream_tests.rs"]
mod tests;
