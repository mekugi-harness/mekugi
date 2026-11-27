//! Incremental model-token-stream projector for authored shell, Python, and
//! Code Mode edits.
//!
//! Projects streaming tool argument deltas (direct `exec_command` JSON and
//! Code Mode `exec` JavaScript calling `tools.exec_command` / `tools.apply_patch`)
//! into `EventMsg::PatchApplyUpdated` so they share the exact same Live Diff
//! rendering pipeline as `apply_patch`.
//!
//! Non-streaming writes whose bytes come from disk rather than the model's
//! token stream (`cat < big > out`, `cp`, `shutil.copy*`, etc.) emit nothing.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use codex_apply_patch::Hunk;
use codex_apply_patch::StreamingPatchParser;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::FileChange;
use codex_protocol::protocol::PatchApplyUpdatedEvent;

use crate::function_tool::FunctionCallError;
use crate::session::turn_context::TurnContext;
use crate::tools::registry::ToolArgumentDiffConsumer;

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
    fn consume_diff(&mut self, turn: &TurnContext, call_id: String, diff: &str) -> Vec<EventMsg> {
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
        self.maybe_emit(call_id).into_iter().collect()
    }

    fn finish(&mut self) -> Result<Vec<EventMsg>, FunctionCallError> {
        if let Some(projector) = self.projector.as_mut() {
            projector.finish();
            if projector.take_dirty()
                && let Some(call_id) = self.call_id.clone()
            {
                let changes = projector.snapshot();
                if !changes.is_empty() || self.last_sent_at.is_some() {
                    self.pending = Some(PatchApplyUpdatedEvent { call_id, changes });
                }
            }
        }
        let event = self.pending.take();
        if event.is_some() {
            self.last_sent_at = Some(Instant::now());
        }
        Ok(event.map(EventMsg::PatchApplyUpdated).into_iter().collect())
    }
}

impl ExecCommandArgumentDiffConsumer {
    fn maybe_emit(&mut self, call_id: String) -> Option<EventMsg> {
        let projector = self.projector.as_mut()?;
        if !projector.take_dirty() {
            return None;
        }
        let changes = projector.snapshot();
        if changes.is_empty() && self.last_sent_at.is_none() {
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
// Code Mode (`exec`) nested edit stream projector
// ---------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct CodeModeEditProjector {
    base_cwd: Option<PathBuf>,
    call_id: Option<String>,
    last_sent_at: Option<Instant>,
    pending: Option<PatchApplyUpdatedEvent>,
    js: JsNestedEditScanner,
    shell: Option<ShellEditProjector>,
    patch_parser: StreamingPatchParser,
    patch_changes: HashMap<PathBuf, FileChange>,
    dirty: bool,
}

impl CodeModeEditProjector {
    pub(crate) fn consume_diff(
        &mut self,
        cwd: &Path,
        call_id: String,
        diff: &str,
    ) -> Option<EventMsg> {
        self.call_id = Some(call_id.clone());
        if self.base_cwd.is_none() {
            self.base_cwd = Some(cwd.to_path_buf());
        }
        let base = self.base_cwd.as_deref().unwrap_or(cwd).to_path_buf();
        let shell = self
            .shell
            .get_or_insert_with(|| ShellEditProjector::new(&base));
        for event in self.js.push(diff) {
            match event {
                JsEditEvent::ExecCmdStart => {
                    shell.reset_script_state();
                }
                JsEditEvent::ExecCmdChunk(chunk) => {
                    shell.push(&chunk);
                }
                JsEditEvent::ExecWorkdir(dir) => {
                    shell.set_workdir(Path::new(&dir));
                }
                JsEditEvent::ExecCmdEnd => {
                    shell.finish();
                }
                JsEditEvent::ApplyPatchStart => {
                    self.patch_parser = StreamingPatchParser::default();
                }
                JsEditEvent::ApplyPatchChunk(chunk) => {
                    if let Ok(hunks) = self.patch_parser.push_delta(&chunk)
                        && !hunks.is_empty()
                    {
                        for (path, change) in hunks_to_file_changes(&hunks, &base) {
                            self.patch_changes.insert(path, change);
                        }
                        self.dirty = true;
                    }
                }
                JsEditEvent::ApplyPatchEnd => {
                    let _ = self.patch_parser.finish();
                }
            }
        }
        if shell.take_dirty() {
            self.dirty = true;
        }
        self.maybe_emit(call_id)
    }

    pub(crate) fn finish(&mut self) -> Option<EventMsg> {
        if let Some(shell) = self.shell.as_mut() {
            shell.finish();
            if shell.take_dirty() {
                self.dirty = true;
            }
        }
        if self.dirty
            && let Some(call_id) = self.call_id.clone()
        {
            self.dirty = false;
            let changes = self.snapshot();
            if !changes.is_empty() || self.last_sent_at.is_some() {
                self.pending = Some(PatchApplyUpdatedEvent { call_id, changes });
            }
        }
        let event = self.pending.take();
        if event.is_some() {
            self.last_sent_at = Some(Instant::now());
        }
        event.map(EventMsg::PatchApplyUpdated)
    }

    fn snapshot(&self) -> HashMap<PathBuf, FileChange> {
        let mut out = self.patch_changes.clone();
        if let Some(shell) = &self.shell {
            out.extend(shell.snapshot());
        }
        out
    }

    fn maybe_emit(&mut self, call_id: String) -> Option<EventMsg> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        let changes = self.snapshot();
        if changes.is_empty() && self.last_sent_at.is_none() {
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

fn hunks_to_file_changes(hunks: &[Hunk], base_cwd: &Path) -> HashMap<PathBuf, FileChange> {
    hunks
        .iter()
        .map(|hunk| {
            let raw = match hunk {
                Hunk::AddFile { path, .. }
                | Hunk::DeleteFile { path }
                | Hunk::UpdateFile { path, .. } => path.as_path(),
            };
            let path = if raw.is_absolute() {
                raw.to_path_buf()
            } else {
                base_cwd.join(raw)
            };
            let change = match hunk {
                Hunk::AddFile { contents, .. } => FileChange::Add {
                    content: contents.clone(),
                },
                Hunk::DeleteFile { .. } => FileChange::Delete {
                    content: String::new(),
                },
                Hunk::UpdateFile {
                    chunks, move_path, ..
                } => {
                    let mut diff = String::new();
                    for chunk in chunks {
                        let old_count = chunk.old_lines.len();
                        let new_count = chunk.new_lines.len();
                        match &chunk.change_context {
                            Some(ctx) => {
                                diff.push_str(&format!(
                                    "@@ -1,{old_count} +1,{new_count} @@ {ctx}\n"
                                ));
                            }
                            None => {
                                diff.push_str(&format!("@@ -1,{old_count} +1,{new_count} @@\n"));
                            }
                        }
                        for line in &chunk.old_lines {
                            diff.push('-');
                            diff.push_str(line);
                            diff.push('\n');
                        }
                        for line in &chunk.new_lines {
                            diff.push('+');
                            diff.push_str(line);
                            diff.push('\n');
                        }
                    }
                    FileChange::Update {
                        unified_diff: diff,
                        move_path: move_path.clone(),
                    }
                }
            };
            (path, change)
        })
        .collect()
}

enum JsEditEvent {
    ExecCmdStart,
    ExecCmdChunk(String),
    ExecWorkdir(String),
    ExecCmdEnd,
    ApplyPatchStart,
    ApplyPatchChunk(String),
    ApplyPatchEnd,
}

#[derive(Default)]
struct JsNestedEditScanner {
    ident: String,
    prev_ident: String,
    after_dot: bool,
    state: JsScanState,
    escape: JsEscape,
    high_surrogate: Option<u16>,
}

#[derive(Default)]
enum JsScanState {
    #[default]
    Normal,
    SkipString {
        quote: char,
    },
    AwaitPatchArg,
    InPatchString {
        quote: char,
    },
    InExecArgs {
        brace_depth: usize,
        expect_value_for: Option<TargetField>,
        pending_key: String,
    },
    InExecFieldString {
        quote: char,
        brace_depth: usize,
        field: TargetField,
        workdir_buf: String,
    },
}

#[derive(Default, Clone, Copy)]
enum JsEscape {
    #[default]
    None,
    Backslash,
    Hex {
        remaining: u8,
        acc: u32,
        is_u16: bool,
    },
}

impl JsNestedEditScanner {
    fn push(&mut self, delta: &str) -> Vec<JsEditEvent> {
        let mut events = Vec::new();
        let mut cmd_buf = String::new();
        let mut patch_buf = String::new();

        for ch in delta.chars() {
            match &mut self.state {
                JsScanState::SkipString { quote } => {
                    let q = *quote;
                    if let Some(_decoded) =
                        feed_js_string_char(&mut self.escape, &mut self.high_surrogate, ch, q)
                    {
                        // skip
                    } else if matches!(self.escape, JsEscape::None) && ch == q {
                        self.state = JsScanState::Normal;
                    }
                    continue;
                }
                JsScanState::InPatchString { quote } => {
                    let q = *quote;
                    if let Some(decoded) =
                        feed_js_string_char(&mut self.escape, &mut self.high_surrogate, ch, q)
                    {
                        patch_buf.push(decoded);
                    } else if matches!(self.escape, JsEscape::None) && ch == q {
                        if !patch_buf.is_empty() {
                            events
                                .push(JsEditEvent::ApplyPatchChunk(std::mem::take(&mut patch_buf)));
                        }
                        events.push(JsEditEvent::ApplyPatchEnd);
                        self.state = JsScanState::Normal;
                    }
                    continue;
                }
                JsScanState::InExecFieldString {
                    quote,
                    brace_depth,
                    field,
                    workdir_buf,
                } => {
                    let q = *quote;
                    let depth = *brace_depth;
                    let f = *field;
                    if let Some(decoded) =
                        feed_js_string_char(&mut self.escape, &mut self.high_surrogate, ch, q)
                    {
                        match f {
                            TargetField::Cmd => cmd_buf.push(decoded),
                            TargetField::Workdir => workdir_buf.push(decoded),
                            TargetField::Other => {}
                        }
                    } else if matches!(self.escape, JsEscape::None) && ch == q {
                        if f == TargetField::Cmd {
                            if !cmd_buf.is_empty() {
                                events
                                    .push(JsEditEvent::ExecCmdChunk(std::mem::take(&mut cmd_buf)));
                            }
                            events.push(JsEditEvent::ExecCmdEnd);
                        } else if f == TargetField::Workdir && !workdir_buf.is_empty() {
                            let dir = std::mem::take(workdir_buf);
                            events.push(JsEditEvent::ExecWorkdir(dir));
                        }
                        self.state = JsScanState::InExecArgs {
                            brace_depth: depth,
                            expect_value_for: None,
                            pending_key: String::new(),
                        };
                    }
                    continue;
                }
                JsScanState::AwaitPatchArg => {
                    if ch.is_whitespace() {
                        continue;
                    }
                    if matches!(ch, '"' | '\'' | '`') {
                        events.push(JsEditEvent::ApplyPatchStart);
                        self.state = JsScanState::InPatchString { quote: ch };
                    } else {
                        self.state = JsScanState::Normal;
                    }
                    continue;
                }
                JsScanState::InExecArgs {
                    brace_depth,
                    expect_value_for,
                    pending_key,
                } => {
                    if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
                        if *brace_depth == 1 && expect_value_for.is_none() {
                            pending_key.push(ch);
                        }
                        continue;
                    }
                    match ch {
                        '{' => {
                            *brace_depth += 1;
                            pending_key.clear();
                        }
                        '}' => {
                            *brace_depth = brace_depth.saturating_sub(1);
                            pending_key.clear();
                            *expect_value_for = None;
                            if *brace_depth == 0 {
                                self.state = JsScanState::Normal;
                            }
                        }
                        ':' if *brace_depth == 1 && expect_value_for.is_none() => {
                            let key = std::mem::take(pending_key);
                            *expect_value_for = Some(match key.as_str() {
                                "cmd" => TargetField::Cmd,
                                "workdir" => TargetField::Workdir,
                                _ => TargetField::Other,
                            });
                        }
                        ',' if *brace_depth == 1 => {
                            pending_key.clear();
                            *expect_value_for = None;
                        }
                        '"' | '\'' | '`' => {
                            let depth = *brace_depth;
                            if depth == 1
                                && let Some(field) = expect_value_for.take()
                            {
                                if field == TargetField::Cmd {
                                    events.push(JsEditEvent::ExecCmdStart);
                                }
                                self.state = JsScanState::InExecFieldString {
                                    quote: ch,
                                    brace_depth: depth,
                                    field,
                                    workdir_buf: String::new(),
                                };
                            } else if depth == 1 && expect_value_for.is_none() {
                                // Quoted key like "cmd": ... — allow chars into pending_key
                            } else {
                                self.state = JsScanState::InExecFieldString {
                                    quote: ch,
                                    brace_depth: depth,
                                    field: TargetField::Other,
                                    workdir_buf: String::new(),
                                };
                            }
                        }
                        _ if ch.is_whitespace() => {}
                        _ => {
                            if *brace_depth == 1 && expect_value_for.is_some() {
                                *expect_value_for = None;
                            }
                        }
                    }
                    continue;
                }
                JsScanState::Normal => {}
            }

            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' {
                self.ident.push(ch);
                continue;
            }
            if !self.ident.is_empty() {
                let name = std::mem::take(&mut self.ident);
                if self.after_dot {
                    self.after_dot = false;
                    if ch == '(' {
                        if self.prev_ident == "tools" && name == "exec_command" {
                            self.prev_ident.clear();
                            self.state = JsScanState::InExecArgs {
                                brace_depth: 0,
                                expect_value_for: None,
                                pending_key: String::new(),
                            };
                            continue;
                        }
                        if self.prev_ident == "tools" && name == "apply_patch" {
                            self.prev_ident.clear();
                            self.state = JsScanState::AwaitPatchArg;
                            continue;
                        }
                    }
                    self.prev_ident = name;
                } else {
                    self.prev_ident = name;
                }
            }
            match ch {
                '.' => {
                    self.after_dot = true;
                }
                '"' | '\'' | '`' => {
                    self.after_dot = false;
                    self.prev_ident.clear();
                    self.state = JsScanState::SkipString { quote: ch };
                }
                '/' => {
                    self.after_dot = false;
                }
                _ if ch.is_whitespace() => {}
                _ => {
                    self.after_dot = false;
                    self.prev_ident.clear();
                }
            }
        }

        if !cmd_buf.is_empty() {
            events.push(JsEditEvent::ExecCmdChunk(cmd_buf));
        }
        if !patch_buf.is_empty() {
            events.push(JsEditEvent::ApplyPatchChunk(patch_buf));
        }
        events
    }
}

fn feed_js_string_char(
    escape: &mut JsEscape,
    high_surrogate: &mut Option<u16>,
    ch: char,
    quote: char,
) -> Option<char> {
    match *escape {
        JsEscape::None => {
            if ch == '\\' {
                *escape = JsEscape::Backslash;
                None
            } else if ch == quote {
                *high_surrogate = None;
                None
            } else {
                *high_surrogate = None;
                Some(ch)
            }
        }
        JsEscape::Backslash => {
            *escape = JsEscape::None;
            match ch {
                'n' => Some('\n'),
                'r' => Some('\r'),
                't' => Some('\t'),
                '\\' => Some('\\'),
                '\'' => Some('\''),
                '"' => Some('"'),
                '`' => Some('`'),
                '$' => Some('$'),
                'x' => {
                    *escape = JsEscape::Hex {
                        remaining: 2,
                        acc: 0,
                        is_u16: false,
                    };
                    None
                }
                'u' => {
                    *escape = JsEscape::Hex {
                        remaining: 4,
                        acc: 0,
                        is_u16: true,
                    };
                    None
                }
                '\n' => None,
                other => Some(other),
            }
        }
        JsEscape::Hex {
            remaining,
            acc,
            is_u16,
        } => {
            if let Some(digit) = ch.to_digit(16) {
                let next = (acc << 4) | digit;
                if remaining > 1 {
                    *escape = JsEscape::Hex {
                        remaining: remaining - 1,
                        acc: next,
                        is_u16,
                    };
                    None
                } else {
                    *escape = JsEscape::None;
                    if is_u16 && (0xD800..=0xDBFF).contains(&next) {
                        *high_surrogate = Some(next as u16);
                        None
                    } else if is_u16 && (0xDC00..=0xDFFF).contains(&next) {
                        if let Some(hi) = high_surrogate.take() {
                            let cp = 0x10000 + (((hi as u32 - 0xD800) << 10) | (next - 0xDC00));
                            char::from_u32(cp)
                        } else {
                            Some('\u{FFFD}')
                        }
                    } else {
                        *high_surrogate = None;
                        char::from_u32(next)
                    }
                }
            } else {
                *escape = JsEscape::None;
                Some(ch)
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
    base_cwd: PathBuf,
    script_cwd: PathBuf,
    mode: ShellMode,
    files: HashMap<PathBuf, StreamedFile>,
    dirty: bool,
}

enum StreamedFile {
    Add { committed: String, partial: String },
    Update { committed: String, partial: String },
}

impl StreamedFile {
    fn to_file_change(&self, extra_tail: Option<&str>) -> Option<FileChange> {
        match self {
            Self::Add { committed, partial } => {
                let tail = extra_tail.unwrap_or("");
                if committed.is_empty() && partial.is_empty() && tail.is_empty() {
                    return None;
                }
                let mut content =
                    String::with_capacity(committed.len() + partial.len() + tail.len());
                content.push_str(committed);
                content.push_str(partial);
                content.push_str(tail);
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
    pub(crate) fn new(base_cwd: &Path) -> Self {
        Self {
            base_cwd: base_cwd.to_path_buf(),
            script_cwd: PathBuf::new(),
            mode: ShellMode::Scan {
                line: String::new(),
            },
            files: HashMap::new(),
            dirty: false,
        }
    }

    pub(crate) fn reset_script_state(&mut self) {
        self.script_cwd.clear();
        self.mode = ShellMode::Scan {
            line: String::new(),
        };
    }

    pub(crate) fn set_workdir(&mut self, dir: &Path) {
        let next = if dir.is_absolute() {
            dir.to_path_buf()
        } else {
            self.base_cwd.join(dir)
        };
        if next != self.base_cwd {
            self.base_cwd = next;
            if !self.files.is_empty() || matches!(self.mode, ShellMode::Heredoc { .. }) {
                self.dirty = true;
            }
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
        let active_heredoc: Option<(&Path, &str)> = match &self.mode {
            ShellMode::Heredoc {
                path,
                delimiter,
                strip_tabs,
                line_buf,
            } => {
                let candidate = if *strip_tabs {
                    line_buf.trim_start_matches('\t')
                } else {
                    line_buf.as_str()
                };
                if !candidate.is_empty() && !delimiter.starts_with(candidate) {
                    Some((path.as_path(), candidate))
                } else {
                    None
                }
            }
            _ => None,
        };

        let mut out = HashMap::new();
        for (raw_path, file) in &self.files {
            let tail = active_heredoc.and_then(|(p, t)| (p == raw_path.as_path()).then_some(t));
            if let Some(change) = file.to_file_change(tail) {
                out.insert(self.resolve_abs(raw_path), change);
            }
        }
        if let Some((raw_path, tail)) = active_heredoc
            && !self.files.contains_key(raw_path)
            && self.files.len() < MAX_STREAM_FILES
        {
            out.insert(
                self.resolve_abs(raw_path),
                FileChange::Add {
                    content: tail.to_owned(),
                },
            );
        }
        out
    }

    fn resolve_abs(&self, raw_path: &Path) -> PathBuf {
        if raw_path.is_absolute() {
            raw_path.to_path_buf()
        } else {
            self.base_cwd.join(raw_path)
        }
    }

    fn resolve_script_target(&self, target: &str) -> PathBuf {
        let p = Path::new(target);
        if p.is_absolute() || self.script_cwd.as_os_str().is_empty() {
            p.to_path_buf()
        } else {
            self.script_cwd.join(p)
        }
    }

    fn push_char(&mut self, ch: char) {
        enum Transition {
            None,
            ToScan,
            ToPythonInline(char, Vec<String>),
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
                    let prefix = &line[..line.len() - ch.len_utf8()];
                    let mut cds = Vec::new();
                    for seg in split_shell_chain(prefix) {
                        let toks = tokenize_shell_segment(seg);
                        if toks.len() == 2
                            && toks[0] == "cd"
                            && !toks[1].starts_with('-')
                            && is_literal_path(&toks[1])
                        {
                            cds.push(strip_shell_quotes(&toks[1]));
                        }
                    }
                    line.clear();
                    transition = Transition::ToPythonInline(quote, cds);
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
                        transition = Transition::ToScan;
                    } else {
                        let path = path.clone();
                        let owned = line.to_owned();
                        self.commit_add_line(&path, &owned, true);
                    }
                } else if line_buf.len() < MAX_STREAM_FILE_BYTES {
                    line_buf.push(ch);
                    let candidate = if *strip_tabs {
                        line_buf.trim_start_matches('\t')
                    } else {
                        line_buf.as_str()
                    };
                    if !delimiter.starts_with(candidate) {
                        self.dirty = true;
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
                } else if line_buf.len() < MAX_STREAM_FILE_BYTES {
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
            Transition::ToPythonInline(quote, cds) => {
                for dir in cds {
                    self.apply_cd(&dir);
                }
                self.mode = ShellMode::PythonInline {
                    quote,
                    escaped: false,
                    py: PythonEditProjector::new(self.script_cwd.clone()),
                };
            }
        }
    }

    fn apply_cd(&mut self, dir: &str) {
        let p = Path::new(dir);
        if p.is_absolute() || self.script_cwd.as_os_str().is_empty() {
            self.script_cwd = p.to_path_buf();
        } else {
            self.script_cwd = self.script_cwd.join(p);
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
                    self.apply_cd(&strip_shell_quotes(&tokens[1]));
                }
                continue;
            }
            if let Some(heredoc) = parse_heredoc_header(&tokens) {
                match heredoc {
                    HeredocHeader::FileWrite {
                        target,
                        delimiter,
                        strip_tabs,
                        overwrite,
                    } => {
                        let path = self.resolve_script_target(&target);
                        if overwrite {
                            self.files.remove(&path);
                        }
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
                            py: PythonEditProjector::new(self.script_cwd.clone()),
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
}

// ---------------------------------------------------------------------------
// Shell header helpers (heredoc + python -c detection)
// ---------------------------------------------------------------------------

enum HeredocHeader {
    FileWrite {
        target: String,
        delimiter: String,
        strip_tabs: bool,
        overwrite: bool,
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
    let mut output_target: Option<(String, bool)> = None;
    let mut positional: Vec<&str> = Vec::new();

    let mut i = 1;
    while i < tokens.len() {
        let tok = tokens[i].as_str();
        if tok == "<" || tok == "0<" || (tok.starts_with('<') && !tok.starts_with("<<")) {
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
        } else if matches!(tok, ">" | ">|" | "1>" | ">>" | "1>>") {
            let overwrite = matches!(tok, ">" | ">|" | "1>");
            i += 1;
            let target = tokens.get(i)?;
            if !is_literal_path(target) || output_target.is_some() {
                return None;
            }
            output_target = Some((strip_shell_quotes(target), overwrite));
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
            let (target, overwrite) = output_target?;
            Some(HeredocHeader::FileWrite {
                target,
                delimiter,
                strip_tabs,
                overwrite,
            })
        }
        "tee" => {
            if output_target.is_some() {
                return None;
            }
            let append = positional.iter().any(|a| *a == "-a" || *a == "--append");
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
                overwrite: !append,
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
    let chars: Vec<char> = segment.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if escaped {
            escaped = false;
            current.push(ch);
            i += 1;
            continue;
        }
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            current.push(ch);
            i += 1;
            continue;
        }
        if in_double {
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_double = false;
            }
            current.push(ch);
            i += 1;
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            i += 1;
            continue;
        }
        if ch == '<' || ch == '>' {
            let mut op = String::new();
            if (current == "0" && ch == '<') || (current == "1" && ch == '>') {
                op.push_str(&std::mem::take(&mut current));
            } else if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            op.push(ch);
            i += 1;
            if i < chars.len() {
                match (ch, chars[i]) {
                    ('<', '<') => {
                        op.push('<');
                        i += 1;
                        if i < chars.len() && chars[i] == '-' {
                            op.push('-');
                            i += 1;
                        }
                    }
                    ('>', '>') | ('>', '|') => {
                        op.push(chars[i]);
                        i += 1;
                    }
                    _ => {}
                }
            }
            tokens.push(op);
            continue;
        }
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
        i += 1;
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
    if !raw.starts_with('\'') && clean.contains(['$', '`', '*', '?', '[']) {
        return false;
    }
    true
}

fn join_rel(script_cwd: &Path, target: &str) -> PathBuf {
    let path = Path::new(target);
    if path.is_absolute() || script_cwd.as_os_str().is_empty() {
        path.to_path_buf()
    } else {
        script_cwd.join(path)
    }
}

// ---------------------------------------------------------------------------
// Incremental Python edit projector (open/read_text -> replace -> write_text)
// ---------------------------------------------------------------------------

struct PythonEditProjector {
    script_cwd: PathBuf,
    path_vars: HashMap<String, PathBuf>,
    file_vars: HashMap<String, PathBuf>,
    text_vars: HashMap<String, PathBuf>,
    call_stack: Vec<PyCallContext>,
    replace_touched: HashSet<PathBuf>,
    written_paths: HashSet<PathBuf>,
    stmt: Vec<PyTok>,
    paren_depth: usize,
    lex: PyLexState,
    active_op: Option<PyStreamOp>,
}

#[derive(Clone)]
enum PyCallContext {
    Neutral,
    ReadOnlySink,
    WriteTarget(PathBuf),
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

#[derive(Clone, Copy)]
enum PyEscape {
    None,
    Backslash,
    Hex { remaining: u8, acc: u32 },
}

enum PyLexState {
    Normal {
        ident: String,
    },
    Comment,
    String {
        quote: char,
        triple: bool,
        raw: bool,
        pending_quotes: u8,
        checking_open: bool,
        escape: PyEscape,
        buf: String,
        streaming: bool,
    },
}

enum PyStreamOp {
    WriteLiteral {
        path: PathBuf,
    },
    ReplaceAwaitOld {
        path: PathBuf,
    },
    ReplaceOld {
        path: PathBuf,
        old: String,
    },
    ReplaceAwaitNew {
        path: PathBuf,
        old: String,
    },
    ReplaceNew {
        path: PathBuf,
        old: String,
        new: String,
    },
}

impl PythonEditProjector {
    fn new(script_cwd: PathBuf) -> Self {
        Self {
            script_cwd,
            path_vars: HashMap::new(),
            file_vars: HashMap::new(),
            text_vars: HashMap::new(),
            call_stack: Vec::new(),
            replace_touched: HashSet::new(),
            written_paths: HashSet::new(),
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
        if self.written_paths.is_empty() {
            for path in self.replace_touched.drain() {
                if files.remove(&path).is_some() {
                    *dirty = true;
                }
            }
        }
    }

    fn in_readonly_sink(&self) -> bool {
        self.call_stack
            .iter()
            .any(|ctx| matches!(ctx, PyCallContext::ReadOnlySink))
    }

    fn enclosing_write_target(&self) -> Option<PathBuf> {
        self.call_stack.iter().rev().find_map(|ctx| match ctx {
            PyCallContext::WriteTarget(path) => Some(path.clone()),
            _ => None,
        })
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
                escape: _,
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
                        let finished = std::mem::take(buf);
                        self.lex = PyLexState::Normal {
                            ident: String::new(),
                        };
                        self.on_string_end(finished, is_streaming, files, dirty);
                        replay_char = Some(ch);
                    } else {
                        *checking_open = false;
                        *pending_quotes = 0;
                    }
                }
                if replay_char.is_none()
                    && let PyLexState::String {
                        quote,
                        triple,
                        raw,
                        pending_quotes,
                        escape,
                        buf,
                        streaming,
                        ..
                    } = &mut self.lex
                {
                    let q = *quote;
                    let is_streaming = *streaming;
                    if !matches!(escape, PyEscape::None) {
                        if let Some(decoded) = advance_py_escape(escape, ch) {
                            push_py_str_slice(
                                buf,
                                &decoded,
                                is_streaming,
                                &mut self.active_op,
                                files,
                                dirty,
                            );
                        }
                        return;
                    }
                    if *triple {
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
                            *escape = PyEscape::Backslash;
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
                        if ch == '\\' && !*raw {
                            *escape = PyEscape::Backslash;
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
                escape: PyEscape::None,
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
                self.call_stack.pop();
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

    fn check_call_open(&mut self) {
        let n = self.stmt.len();
        let mut ctx = PyCallContext::Neutral;
        if n >= 2
            && let PyTok::Ident(func) = &self.stmt[n - 2]
            && (n < 3 || self.stmt[n - 3] != PyTok::Dot)
            && matches!(func.as_str(), "print" | "len" | "repr" | "str" | "hash")
        {
            ctx = PyCallContext::ReadOnlySink;
        } else if n >= 3
            && let PyTok::Ident(method) = &self.stmt[n - 2]
            && self.stmt[n - 3] == PyTok::Dot
        {
            let receiver_tokens = &self.stmt[..n - 3];
            match method.as_str() {
                "write_text" => {
                    if let Some(path) = self.resolve_path_expr(receiver_tokens) {
                        self.written_paths.insert(path.clone());
                        ctx = PyCallContext::WriteTarget(path.clone());
                        self.active_op = Some(PyStreamOp::WriteLiteral { path });
                    }
                }
                "write" => {
                    if let Some(path) = self.resolve_file_expr(receiver_tokens) {
                        self.written_paths.insert(path.clone());
                        ctx = PyCallContext::WriteTarget(path.clone());
                        self.active_op = Some(PyStreamOp::WriteLiteral { path });
                    }
                }
                "replace" => {
                    if !self.in_readonly_sink() {
                        let target = self
                            .enclosing_write_target()
                            .or_else(|| self.resolve_text_expr(receiver_tokens));
                        if let Some(path) = target {
                            self.replace_touched.insert(path.clone());
                            self.active_op = Some(PyStreamOp::ReplaceAwaitOld { path });
                        }
                    }
                }
                _ => {}
            }
        }
        self.call_stack.push(ctx);
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
            PyTok::RParen => extract_path_call(tokens, &self.script_cwd),
            _ => None,
        }
    }

    fn resolve_file_expr(&self, tokens: &[PyTok]) -> Option<PathBuf> {
        match tokens.last()? {
            PyTok::Ident(name) => self.file_vars.get(name).cloned(),
            PyTok::RParen => extract_open_call(tokens, &self.script_cwd, &self.path_vars),
            _ => None,
        }
    }

    fn resolve_text_expr(&self, tokens: &[PyTok]) -> Option<PathBuf> {
        match tokens.last()? {
            PyTok::Ident(name) => self.text_vars.get(name).cloned(),
            PyTok::RParen => extract_read_text_chain(
                tokens,
                &self.script_cwd,
                &self.path_vars,
                &self.file_vars,
                &self.text_vars,
            ),
            _ => None,
        }
    }

    fn eval_statement(&mut self) {
        let stmt = std::mem::take(&mut self.stmt);
        if stmt.is_empty() {
            return;
        }
        if matches!(stmt.first(), Some(PyTok::Ident(k)) if k == "with")
            && let Some(as_pos) = stmt
                .iter()
                .position(|t| matches!(t, PyTok::Ident(k) if k == "as"))
            && let Some(PyTok::Ident(var)) = stmt.get(as_pos + 1)
            && let Some(path) =
                extract_open_call(&stmt[1..as_pos], &self.script_cwd, &self.path_vars)
        {
            self.file_vars.insert(var.clone(), path);
            return;
        }

        if let [PyTok::Ident(var), PyTok::Eq, rhs @ ..] = stmt.as_slice() {
            if let [PyTok::Str(lit)] = rhs {
                self.path_vars
                    .insert(var.clone(), join_rel(&self.script_cwd, lit));
            } else if let Some(path) = extract_path_call(rhs, &self.script_cwd) {
                self.path_vars.insert(var.clone(), path);
            } else if let Some(path) = extract_open_call(rhs, &self.script_cwd, &self.path_vars) {
                self.file_vars.insert(var.clone(), path);
            } else if let Some(path) = extract_read_text_chain(
                rhs,
                &self.script_cwd,
                &self.path_vars,
                &self.file_vars,
                &self.text_vars,
            ) {
                self.text_vars.insert(var.clone(), path);
            }
        }
    }
}

fn advance_py_escape(state: &mut PyEscape, ch: char) -> Option<String> {
    match *state {
        PyEscape::None => None,
        PyEscape::Backslash => {
            *state = PyEscape::None;
            match ch {
                'n' => Some("\n".to_string()),
                'r' => Some("\r".to_string()),
                't' => Some("\t".to_string()),
                '\\' => Some("\\".to_string()),
                '\'' => Some("'".to_string()),
                '"' => Some("\"".to_string()),
                'a' => Some("\u{0007}".to_string()),
                'b' => Some("\u{0008}".to_string()),
                'f' => Some("\u{000C}".to_string()),
                'v' => Some("\u{000B}".to_string()),
                '0' => Some("\0".to_string()),
                '\n' => None,
                'x' => {
                    *state = PyEscape::Hex {
                        remaining: 2,
                        acc: 0,
                    };
                    None
                }
                'u' => {
                    *state = PyEscape::Hex {
                        remaining: 4,
                        acc: 0,
                    };
                    None
                }
                'U' => {
                    *state = PyEscape::Hex {
                        remaining: 8,
                        acc: 0,
                    };
                    None
                }
                other => Some(format!("\\{other}")),
            }
        }
        PyEscape::Hex { remaining, acc } => {
            if let Some(digit) = ch.to_digit(16) {
                let next = (acc << 4) | digit;
                if remaining > 1 {
                    *state = PyEscape::Hex {
                        remaining: remaining - 1,
                        acc: next,
                    };
                    None
                } else {
                    *state = PyEscape::None;
                    char::from_u32(next).map(|c| c.to_string())
                }
            } else {
                *state = PyEscape::None;
                Some(ch.to_string())
            }
        }
    }
}

fn extract_path_call(tokens: &[PyTok], script_cwd: &Path) -> Option<PathBuf> {
    let n = tokens.len();
    if n >= 4
        && matches!(&tokens[n - 4], PyTok::Ident(name) if name == "Path")
        && tokens[n - 3] == PyTok::LParen
        && let PyTok::Str(path) = &tokens[n - 2]
        && tokens[n - 1] == PyTok::RParen
    {
        return Some(join_rel(script_cwd, path));
    }
    None
}

fn extract_open_call(
    tokens: &[PyTok],
    script_cwd: &Path,
    path_vars: &HashMap<String, PathBuf>,
) -> Option<PathBuf> {
    let open_pos = tokens
        .iter()
        .rposition(|t| matches!(t, PyTok::Ident(name) if name == "open"))?;
    if tokens.get(open_pos + 1) != Some(&PyTok::LParen) {
        return None;
    }
    match tokens.get(open_pos + 2)? {
        PyTok::Str(path) => Some(join_rel(script_cwd, path)),
        PyTok::Ident(var) => path_vars.get(var).cloned(),
        _ => None,
    }
}

fn extract_read_text_chain(
    tokens: &[PyTok],
    script_cwd: &Path,
    path_vars: &HashMap<String, PathBuf>,
    file_vars: &HashMap<String, PathBuf>,
    text_vars: &HashMap<String, PathBuf>,
) -> Option<PathBuf> {
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
                    if let Some(p) = extract_path_call(recv, script_cwd) {
                        return Some(p);
                    }
                }
                "read" => {
                    if let Some(PyTok::Ident(var)) = recv.last()
                        && let Some(p) = file_vars.get(var)
                    {
                        return Some(p.clone());
                    }
                    if let Some(p) = extract_open_call(recv, script_cwd, path_vars) {
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

fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.strip_suffix('\n').unwrap_or(text).split('\n').count()
    }
}

fn format_replace_hunk(old: &str, new: &str) -> String {
    let old_count = count_lines(old);
    let new_count = count_lines(new);
    let mut out = format!("@@ -1,{old_count} +1,{new_count} @@\n");
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
