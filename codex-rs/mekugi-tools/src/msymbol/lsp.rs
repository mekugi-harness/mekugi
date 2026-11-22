use std::collections::HashMap;
use std::collections::HashSet;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use serde_json::Value;
use serde_json::json;

use super::QueryMode;
use super::RETAINED_BYTES;
use super::SymbolLocation;
use super::SymbolQuery;
use super::SymbolRange;
use super::rust;
use super::symbol_file_uri;
use super::symbol_uri_path;
use crate::structure::Position;

const RESOLVER_TIMEOUT: Duration = Duration::from_secs(30);
const RESOLVER_DRAIN: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub(crate) enum RpcEvent {
    Message(Value),
    Error(String),
    Eof,
}

#[derive(Clone, Debug)]
pub(crate) struct RustServerStatus {
    pub health: String,
    pub quiescent: bool,
    pub message: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct RustDiagnosticReport {
    pub kind: String,
    #[serde(default)]
    pub items: Vec<RustDiagnosticItem>,
    #[serde(default, rename = "relatedDocuments")]
    pub related_documents: HashMap<String, Option<RustDiagnosticReport>>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct RustDiagnosticItem {
    pub range: RustRange,
    #[serde(default)]
    pub severity: i64,
    pub code: Option<Value>,
    #[serde(default)]
    pub message: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub(crate) struct RustRange {
    pub start: Position,
    pub end: Position,
}

pub(crate) struct LspClient<W: Write> {
    input: W,
    events: mpsc::Receiver<RpcEvent>,
    next_id: i64,
    folders: Value,
    pub(crate) configuration: Value,
    pub(crate) rust_status: Option<RustServerStatus>,
    pub(crate) rust_revision: u64,
    pub(crate) rust_reports: HashMap<String, RustDiagnosticReport>,
}

impl<W: Write> LspClient<W> {
    pub(crate) fn write_msg(&mut self, value: &Value) -> Result<(), String> {
        let data = serde_json::to_vec(value).map_err(|e| e.to_string())?;
        write!(self.input, "Content-Length: {}\r\n\r\n", data.len()).map_err(|e| e.to_string())?;
        self.input.write_all(&data).map_err(|e| e.to_string())?;
        self.input.flush().map_err(|e| e.to_string())
    }

    pub(crate) fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), String> {
        let msg = match params {
            Some(p) => json!({"jsonrpc": "2.0", "method": method, "params": p}),
            None => json!({"jsonrpc": "2.0", "method": method}),
        };
        self.write_msg(&msg)
    }

    pub(crate) fn handle_server_message(&mut self, msg: &Value) -> Result<(), String> {
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        if method == "experimental/serverStatus"
            && let Some(params) = msg.get("params")
        {
            let status = RustServerStatus {
                health: params
                    .get("health")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                quiescent: params
                    .get("quiescent")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                message: params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            };
            let unstable = !status.quiescent || status.health != "ok";
            self.rust_status = Some(status);
            if unstable {
                self.rust_revision += 1;
                self.rust_reports.clear();
            }
        }

        if let Some(id) = msg.get("id") {
            let response = match method {
                "workspace/configuration" => {
                    let items = msg
                        .get("params")
                        .and_then(|p| p.get("items"))
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let is_rust = self.configuration.get("cargo").is_some();
                    let settings: Vec<Value> = items
                        .iter()
                        .map(|item| {
                            let section = item.get("section").and_then(Value::as_str).unwrap_or("");
                            if is_rust && !section.is_empty() && section != "rust-analyzer" {
                                if let Some(rest) = section.strip_prefix("rust-analyzer.") {
                                    let mut cur = &self.configuration;
                                    for part in rest.split('.') {
                                        if let Some(next) = cur.get(part) {
                                            cur = next;
                                        } else {
                                            return Value::Null;
                                        }
                                    }
                                    cur.clone()
                                } else {
                                    Value::Null
                                }
                            } else {
                                self.configuration.clone()
                            }
                        })
                        .collect();
                    json!({"jsonrpc": "2.0", "id": id, "result": settings})
                }
                "workspace/workspaceFolders" => {
                    json!({"jsonrpc": "2.0", "id": id, "result": self.folders})
                }
                "client/registerCapability" | "window/workDoneProgress/create" => {
                    json!({"jsonrpc": "2.0", "id": id, "result": Value::Null})
                }
                _ => {
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32601, "message": "method not supported"}
                    })
                }
            };
            self.write_msg(&response)?;
        }
        Ok(())
    }

    pub(crate) fn recv_event(&self, deadline: Instant) -> Result<RpcEvent, String> {
        let now = Instant::now();
        if now >= deadline {
            return Err("deadline_exceeded".to_string());
        }
        match self.events.recv_timeout(deadline - now) {
            Ok(ev) => Ok(ev),
            Err(mpsc::RecvTimeoutError::Timeout) => Err("deadline_exceeded".to_string()),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(RpcEvent::Eof),
        }
    }

    pub(crate) fn request(
        &mut self,
        deadline: Instant,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, String> {
        if Instant::now() >= deadline {
            return Err("deadline_exceeded".to_string());
        }
        self.next_id += 1;
        let id = self.next_id;
        let msg = match params {
            Some(p) => json!({"jsonrpc": "2.0", "id": id, "method": method, "params": p}),
            None => json!({"jsonrpc": "2.0", "id": id, "method": method}),
        };
        self.write_msg(&msg)?;

        loop {
            match self.recv_event(deadline)? {
                RpcEvent::Eof => {
                    return Err("language server exited before replying: EOF".to_string());
                }
                RpcEvent::Error(err) => {
                    return Err(format!("language server protocol: {err}"));
                }
                RpcEvent::Message(msg) => {
                    if msg
                        .get("method")
                        .and_then(Value::as_str)
                        .is_some_and(|m| !m.is_empty())
                    {
                        self.handle_server_message(&msg)?;
                        continue;
                    }
                    let resp_id = msg.get("id").and_then(Value::as_i64);
                    if resp_id != Some(id) {
                        continue;
                    }
                    if let Some(err) = msg.get("error") {
                        let message = err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("LSP error");
                        return Err(message.to_string());
                    }
                    match msg.get("result") {
                        Some(result) => return Ok(result.clone()),
                        None => return Err("LSP response has no result".to_string()),
                    }
                }
            }
        }
    }
}

const MAX_LSP_HEADER_LINE_BYTES: usize = 8192;
const RPC_EVENT_CHANNEL_CAPACITY: usize = 256;

fn read_bounded_header_line<R: BufRead>(
    reader: &mut R,
    limit: usize,
) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let mut limited = reader.take((limit + 1) as u64);
    let read = limited.read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Ok(None);
    }
    if buf.len() > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "oversized LSP header",
        ));
    }
    String::from_utf8(buf).map(Some).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid UTF-8 in LSP header",
        )
    })
}

fn read_rpc_loop<R: Read>(reader: R, tx: &mpsc::SyncSender<RpcEvent>) {
    let mut br = BufReader::with_capacity(8192, reader);
    loop {
        let mut content_length: Option<usize> = None;
        loop {
            match read_bounded_header_line(&mut br, MAX_LSP_HEADER_LINE_BYTES) {
                Ok(None) => {
                    let _ = tx.send(RpcEvent::Eof);
                    return;
                }
                Ok(Some(line)) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some((key, val)) = trimmed.split_once(':')
                        && key.eq_ignore_ascii_case("Content-Length")
                    {
                        content_length = val.trim().parse::<usize>().ok();
                    }
                }
                Err(err) => {
                    let _ = tx.send(RpcEvent::Error(err.to_string()));
                    return;
                }
            }
        }

        let Some(length) = content_length.filter(|&len| len <= RETAINED_BYTES) else {
            let _ = tx.send(RpcEvent::Error("invalid LSP Content-Length".to_string()));
            return;
        };

        let mut buf = vec![0u8; length];
        if let Err(err) = br.read_exact(&mut buf) {
            let _ = tx.send(RpcEvent::Error(err.to_string()));
            return;
        }
        match serde_json::from_slice::<Value>(&buf) {
            Ok(msg) => {
                if tx.send(RpcEvent::Message(msg)).is_err() {
                    return;
                }
            }
            Err(err) => {
                let _ = tx.send(RpcEvent::Error(format!("invalid LSP message: {err}")));
                return;
            }
        }
    }
}

pub(crate) fn configure_process_group(cmd: &mut Command, _dedicated_process: bool) {
    crate::common::configure_new_process_group(cmd);
}

pub(crate) fn retire_child(child: &mut Child, _dedicated_process: bool) {
    crate::common::retire_child_process_group(child);
}

pub(crate) fn prepare_frontend_cleanup() -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let rc = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    Ok(())
}

pub(crate) fn cleanup_frontend_orphans() {
    #[cfg(target_os = "linux")]
    {
        let my_pid = std::process::id() as i32;
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            let Ok(entries) = std::fs::read_dir("/proc") else {
                return;
            };
            let mut found = false;
            for entry in entries.flatten() {
                let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
                    continue;
                };
                if pid <= 0 || !is_direct_child_of(pid, my_pid) {
                    continue;
                }
                found = true;
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                    let mut status: libc::c_int = 0;
                    libc::waitpid(pid, &mut status, libc::WNOHANG);
                }
            }
            if !found {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(target_os = "linux")]
fn is_direct_child_of(pid: i32, parent: i32) -> bool {
    let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) else {
        return false;
    };
    for line in status.lines() {
        if let Some(val) = line.strip_prefix("PPid:") {
            return val.trim().parse::<i32>().ok() == Some(parent);
        }
    }
    false
}

pub(crate) fn resolver_error(deadline: Instant, resolver: &str, err: String) -> String {
    if err == "deadline_exceeded" || Instant::now() >= deadline {
        let hint = if resolver == "rust" {
            "keep the Cargo workspace root; try unfinished batch targets separately (a single cold query may still time out)"
        } else {
            "retry with a narrower --workspace ROOT"
        };
        return format!("resolver exceeded the 30 s limit; {hint}");
    }
    err
}

pub(crate) fn resolver_start_error(name: &str, err: &std::io::Error) -> String {
    if err.kind() == std::io::ErrorKind::NotFound {
        let hint = if name == "tsc" {
            "expose TypeScript 7 tsc with --lsp support on the executor PATH".to_string()
        } else {
            format!("expose {name} on the executor PATH")
        };
        return format!("{name} is unavailable; {hint}");
    }
    format!("cannot start {name}: {err}")
}

fn parse_locations(raw: &Value, mode: QueryMode) -> Result<Vec<SymbolLocation>, String> {
    if raw.is_null() {
        return Ok(Vec::new());
    }
    let items: Vec<&Value> = if let Some(arr) = raw.as_array() {
        arr.iter().collect()
    } else if mode == QueryMode::Def && raw.is_object() {
        vec![raw]
    } else {
        return Err("invalid references response".to_string());
    };

    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let uri = item
            .get("uri")
            .or_else(|| item.get("targetUri"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let selected = item
            .get("targetSelectionRange")
            .or_else(|| item.get("range"))
            .or_else(|| item.get("targetRange"));
        let Some(range_obj) = selected else {
            return Err("invalid language-server location".to_string());
        };
        let parse_pos = |obj: &Value| -> Option<Position> {
            let line = obj.get("line")?.as_i64()?;
            let ch = obj.get("character")?.as_i64()?;
            if line < 0 || ch < 0 {
                return None;
            }
            Some(Position {
                line: line as usize,
                character: ch as usize,
            })
        };
        let (Some(start), Some(end)) = (
            range_obj.get("start").and_then(parse_pos),
            range_obj.get("end").and_then(parse_pos),
        ) else {
            return Err("invalid language-server location".to_string());
        };
        if uri.is_empty() {
            return Err("invalid language-server location".to_string());
        }
        match symbol_uri_path(uri) {
            Ok(path) => out.push(SymbolLocation {
                outside: false,
                path,
                range: SymbolRange { start, end },
            }),
            Err(()) => out.push(SymbolLocation {
                outside: true,
                path: std::path::PathBuf::new(),
                range: SymbolRange { start, end },
            }),
        }
    }
    Ok(out)
}

pub(crate) fn run_lsp(
    root: &Path,
    resolver: &str,
    scratch: &Path,
    queries: &mut [&mut SymbolQuery],
    dedicated_process: bool,
    path_override: Option<&std::ffi::OsStr>,
) {
    let deadline = Instant::now() + RESOLVER_TIMEOUT;
    let (name, args): (&str, &[&str]) = match resolver {
        "rust" => ("rust-analyzer", &[]),
        "python" => ("pyright-langserver", &["--stdio"]),
        "gopls" => ("gopls", &["serve"]),
        _ => ("tsc", &["--lsp", "--stdio"]),
    };

    let fail_all = |queries: &mut [&mut SymbolQuery], err: String| {
        let msg = resolver_error(deadline, resolver, err);
        for q in queries.iter_mut() {
            if q.err.is_none() {
                q.err = Some(msg.clone());
            }
        }
    };

    let rust_analysis = if resolver == "rust" {
        match rust::prepare_rust(
            deadline,
            dedicated_process,
            root,
            scratch,
            queries,
            path_override,
        ) {
            Ok(cfg) => Some(cfg),
            Err(err) => {
                fail_all(queries, err);
                return;
            }
        }
    } else {
        None
    };

    let resolved_bin = resolve_binary(name, path_override);
    let mut cmd = Command::new(&resolved_bin);
    cmd.args(args)
        .current_dir(root)
        .env("TMPDIR", scratch)
        .env("TMP", scratch)
        .env("TEMP", scratch)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(p) = path_override {
        cmd.env("PATH", p);
    }
    if let Some(ref rust_cfg) = rust_analysis {
        cmd.env("CARGO_NET_OFFLINE", "true")
            .env("RUSTUP_AUTO_INSTALL", "0")
            .env("RUSTUP", &rust_cfg.rustup);
    }
    configure_process_group(&mut cmd, dedicated_process);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => {
            fail_all(queries, resolver_start_error(name, &err));
            return;
        }
    };

    let Some(stdin) = child.stdin.take() else {
        retire_child(&mut child, dedicated_process);
        fail_all(queries, "failed to open LSP stdin".to_string());
        return;
    };
    let Some(stdout) = child.stdout.take() else {
        retire_child(&mut child, dedicated_process);
        fail_all(queries, "failed to open LSP stdout".to_string());
        return;
    };
    let stderr_pipe = child.stderr.take();

    let child_pid = child.id() as i32;
    let (watchdog_tx, watchdog_rx) = mpsc::sync_channel::<()>(1);
    let watchdog_handle = std::thread::spawn(move || {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if matches!(
            watchdog_rx.recv_timeout(remaining),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            crate::common::kill_process_group(child_pid);
        }
    });

    let (tx, rx) = mpsc::sync_channel::<RpcEvent>(RPC_EVENT_CHANNEL_CAPACITY);
    let _reader_thread = std::thread::spawn(move || {
        read_rpc_loop(stdout, &tx);
    });

    let (stderr_rx, stderr_thread) = crate::common::spawn_bounded_reader(move || {
        let mut buf = Vec::new();
        if let Some(mut pipe) = stderr_pipe {
            let mut chunk = [0u8; 8192];
            loop {
                match pipe.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let available = (RETAINED_BYTES + 1).saturating_sub(buf.len());
                        if available > 0 {
                            buf.extend_from_slice(&chunk[..n.min(available)]);
                        }
                    }
                }
            }
        }
        buf
    });

    let folder_name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "workspace".to_string());
    let folders = json!([{"uri": symbol_file_uri(root), "name": folder_name}]);

    let configuration = if resolver == "gopls" {
        json!({"staticcheck": false})
    } else if let Some(ref rust_cfg) = rust_analysis {
        rust_cfg.config.clone()
    } else {
        Value::Null
    };

    let mut lsp = LspClient {
        input: stdin,
        events: rx,
        next_id: 0,
        folders: folders.clone(),
        configuration,
        rust_status: None,
        rust_revision: 0,
        rust_reports: HashMap::new(),
    };

    let mut init_params = json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "mekugi", "version": "1"},
        "rootUri": symbol_file_uri(root),
        "workspaceFolders": folders,
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "workspace": {"configuration": true, "workspaceFolders": true},
            "textDocument": {}
        }
    });
    if resolver == "gopls" {
        init_params["initializationOptions"] = lsp.configuration.clone();
    } else if resolver == "rust" {
        init_params["initializationOptions"] = lsp.configuration.clone();
        init_params["capabilities"]["experimental"] = json!({"serverStatusNotification": true});
        init_params["capabilities"]["textDocument"] =
            json!({"diagnostic": {"dynamicRegistration": false}});
    }

    let initialized = lsp.request(deadline, "initialize", Some(init_params));
    match initialized {
        Err(err) => {
            fail_all(queries, err);
        }
        Ok(init_result) => {
            let encoding = init_result
                .get("capabilities")
                .and_then(|c| c.get("positionEncoding"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !encoding.is_empty() && encoding != "utf-16" {
                fail_all(
                    queries,
                    format!("unsupported language-server position encoding {encoding:?}"),
                );
            } else if let Err(err) = lsp.notify("initialized", Some(json!({}))) {
                fail_all(queries, err);
            } else {
                let has_pull_diag = init_result
                    .get("capabilities")
                    .and_then(|c| c.get("diagnosticProvider"))
                    .is_some_and(|v| !v.is_null() && v != &Value::Bool(false));
                if resolver == "rust" && !has_pull_diag {
                    fail_all(
                        queries,
                        rust::rust_coverage_msg(
                            "installed rust-analyzer lacks native pull diagnostics",
                        ),
                    );
                } else {
                    execute_queries(
                        &mut lsp,
                        deadline,
                        dedicated_process,
                        root,
                        resolver,
                        scratch,
                        rust_analysis.as_ref(),
                        path_override,
                        queries,
                    );
                }
            }
        }
    }

    // Shutdown gracefully within grace window, then terminate process group.
    let grace = if resolver == "gopls" {
        Duration::from_millis(100)
    } else {
        RESOLVER_DRAIN
    };
    let shutdown_deadline = Instant::now() + grace;
    let _ = lsp.request(shutdown_deadline, "shutdown", None);
    let _ = lsp.notify("exit", None);
    drop(lsp);

    let wait_deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < wait_deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                retire_child(&mut child, dedicated_process);
                break;
            }
        }
    }
    retire_child(&mut child, dedicated_process);
    let _ = watchdog_tx.send(());
    let _ = watchdog_handle.join();

    if let Some(stderr_bytes) =
        crate::common::recv_reader_with_timeout(stderr_rx, stderr_thread, Duration::from_secs(2))
    {
        if stderr_bytes.len() > RETAINED_BYTES {
            for q in queries.iter_mut() {
                if q.err.is_none() {
                    q.err = Some("resolver stderr exceeds the 16 MiB bound".to_string());
                }
            }
            return;
        }
        let stderr_str = String::from_utf8_lossy(&stderr_bytes);
        let mut filtered = String::new();
        for line in stderr_str.split_inclusive('\n') {
            let trim = line.trim();
            if trim != "context canceled" && trim != "error handling method 'exit': EOF" {
                filtered.push_str(line);
            }
        }
        if let Some(first) = queries.first_mut() {
            first.stderr = filtered;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_queries<W: Write>(
    lsp: &mut LspClient<W>,
    deadline: Instant,
    dedicated_process: bool,
    root: &Path,
    resolver: &str,
    scratch: &Path,
    rust_analysis: Option<&rust::RustAnalysis>,
    path_override: Option<&std::ffi::OsStr>,
    queries: &mut [&mut SymbolQuery],
) {
    let mut opened: HashSet<String> = HashSet::new();
    let mut custom_build = false;
    let mut checked_go_flags = false;
    let mut infer_go_tags = false;

    for (q_idx, q) in queries.iter_mut().enumerate() {
        if q.err.is_some() {
            continue;
        }
        if Instant::now() >= deadline {
            q.err = Some(resolver_error(
                deadline,
                resolver,
                "deadline_exceeded".to_string(),
            ));
            continue;
        }

        if custom_build {
            lsp.configuration["buildFlags"] = json!([]);
            if let Err(err) = lsp.notify("workspace/didChangeConfiguration", Some(json!({}))) {
                q.err = Some(resolver_error(deadline, resolver, err));
                continue;
            }
            custom_build = false;
        }

        let Some(file) = q.file.as_ref() else {
            continue;
        };
        let uri = symbol_file_uri(&file.path);
        if opened.insert(uri.clone()) {
            let format = file.parsed.format;
            let mut language = if format.kind == "json" {
                "json".to_string()
            } else {
                format.language.unwrap_or("").to_string()
            };
            if format.jsx {
                language.push_str("react");
            }
            let open_params = json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language,
                    "version": 1,
                    "text": file.parsed.source,
                }
            });
            if let Err(err) = lsp.notify("textDocument/didOpen", Some(open_params)) {
                q.err = Some(resolver_error(deadline, resolver, err));
                continue;
            }
        }

        let position = file.parsed.lines.position(q.offset);
        let (method, params) = match q.mode {
            QueryMode::Def => (
                "textDocument/definition",
                json!({
                    "textDocument": {"uri": uri},
                    "position": position,
                }),
            ),
            QueryMode::Refs => (
                "textDocument/references",
                json!({
                    "textDocument": {"uri": uri},
                    "position": position,
                    "context": {"includeDeclaration": true},
                }),
            ),
        };

        if resolver == "rust" {
            if let Err(err) = rust::rust_ready(lsp, deadline) {
                q.err = Some(resolver_error(deadline, resolver, err));
                continue;
            }
            let coverage_revision = lsp.rust_revision;
            if q.mode == QueryMode::Refs
                && q.coverage.is_none()
                && let Some(ra) = rust_analysis
                && let Some(sources) = ra.sources.get(&q_idx)
            {
                for path in sources {
                    if let Err(err) =
                        rust::rust_diagnostics(lsp, deadline, &symbol_file_uri(path), None)
                    {
                        q.coverage = Some(format!("{}: {err}", path.display()));
                        break;
                    }
                }
            }
            if q.coverage.is_none()
                && let Err(err) = rust::rust_diagnostics(lsp, deadline, &uri, Some(position))
            {
                q.coverage = Some(err);
            }
            if let Err(err) = rust::rust_ready(lsp, deadline) {
                q.err = Some(resolver_error(deadline, resolver, err));
                continue;
            }
            if lsp.rust_revision != coverage_revision {
                lsp.rust_reports.clear();
                q.err = Some(rust::rust_coverage_msg(
                    "analysis changed during coverage acquisition; retry after loading settles",
                ));
                continue;
            }
        }

        let revision = lsp.rust_revision;
        let mut response = lsp.request(deadline, method, Some(params.clone()));

        if resolver == "rust" && response.is_ok() {
            let mut post_err: Option<String> = None;
            if let Err(err) = rust::rust_ready(lsp, deadline) {
                post_err = Some(err);
            }
            if q.coverage.is_none() {
                match lsp.request(deadline, "textDocument/hover", Some(params.clone())) {
                    Ok(hover) if !hover.is_null() => {}
                    _ => {
                        q.coverage = Some(rust::rust_coverage_msg(
                            "selected token has no active semantic information",
                        ));
                    }
                }
            }
            if let Err(err) = rust::rust_ready(lsp, deadline) {
                post_err = Some(err);
            }
            if lsp.rust_revision != revision {
                post_err = Some(rust::rust_coverage_msg(
                    "analysis changed during semantic query; retry after loading settles",
                ));
            }
            if let Some(err) = post_err {
                response = Err(err);
            }
        }

        if resolver == "gopls"
            && let Err(ref err) = response
            && err.contains("no package metadata for file")
            && let Some(tag) = missing_go_build_tag(&file.parsed.source)
        {
            if !checked_go_flags {
                infer_go_tags = check_goflags_allows_tag_inference(
                    root,
                    scratch,
                    deadline,
                    dedicated_process,
                    path_override,
                );
                checked_go_flags = true;
            }
            if Instant::now() >= deadline {
                q.err = Some(resolver_error(
                    deadline,
                    resolver,
                    "deadline_exceeded".to_string(),
                ));
                continue;
            }
            if infer_go_tags {
                lsp.configuration["buildFlags"] = json!([format!("-tags={tag}")]);
                custom_build = true;
                match lsp.notify("workspace/didChangeConfiguration", Some(json!({}))) {
                    Ok(()) => {
                        response = lsp.request(deadline, method, Some(params));
                    }
                    Err(notify_err) => {
                        response = Err(notify_err);
                    }
                }
            }
        }

        match response {
            Ok(raw) => match parse_locations(&raw, q.mode) {
                Ok(locs) => q.locations = locs,
                Err(err) => q.err = Some(err),
            },
            Err(err) => {
                q.err = Some(resolver_error(deadline, resolver, err));
            }
        }
    }
}

pub(crate) fn resolve_binary(
    name: &str,
    path_override: Option<&std::ffi::OsStr>,
) -> std::path::PathBuf {
    if let Some(paths) = path_override {
        for dir in std::env::split_paths(paths) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    std::path::PathBuf::from(name)
}

fn check_goflags_allows_tag_inference(
    root: &Path,
    scratch: &Path,
    deadline: Instant,
    dedicated_process: bool,
    path_override: Option<&std::ffi::OsStr>,
) -> bool {
    let mut cmd = Command::new(resolve_binary("go", path_override));
    cmd.args(["env", "GOFLAGS"])
        .current_dir(root)
        .env("TMPDIR", scratch)
        .env("TMP", scratch)
        .env("TEMP", scratch)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(p) = path_override {
        cmd.env("PATH", p);
    }
    configure_process_group(&mut cmd, dedicated_process);
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    let Some(stdout) = child.stdout.take() else {
        retire_child(&mut child, dedicated_process);
        return false;
    };
    let (read_rx, read_handle) = crate::common::spawn_bounded_reader(move || {
        let mut out = Vec::new();
        let _ = stdout
            .take((RETAINED_BYTES + 1) as u64)
            .read_to_end(&mut out);
        out
    });
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                retire_child(&mut child, dedicated_process);
                break false;
            }
        }
    };
    crate::common::kill_process_group(child.id() as i32);
    if !ok {
        return false;
    }
    let drain_budget = deadline
        .saturating_duration_since(Instant::now())
        .min(Duration::from_secs(2));
    let Some(bytes) = crate::common::recv_reader_with_timeout(read_rx, read_handle, drain_budget)
    else {
        return false;
    };
    if bytes.len() > RETAINED_BYTES {
        return false;
    }
    can_infer_go_tags(&String::from_utf8_lossy(&bytes))
}

/// Extract a single positive `//go:build <tag>` constraint before `package`.
pub(crate) fn missing_go_build_tag(source: &str) -> Option<String> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("package ") {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("//go:build") {
            if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
                continue;
            }
            let expr = rest.trim();
            if is_simple_go_build_tag(expr) {
                return Some(expr.to_string());
            }
            return None;
        }
    }
    None
}

fn is_simple_go_build_tag(expr: &str) -> bool {
    !expr.is_empty()
        && expr
            .chars()
            .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '.')
}

/// Check `GOFLAGS` using `cmd/internal/quoted.Split` quoting rules to ensure no `-tags` is set.
pub(crate) fn can_infer_go_tags(mut flags: &str) -> bool {
    loop {
        flags = flags.trim_start_matches([' ', '\t', '\r', '\n']);
        if flags.is_empty() {
            return true;
        }
        let field = if flags.starts_with('\'') || flags.starts_with('"') {
            let quote = flags.as_bytes()[0] as char;
            let rest = &flags[1..];
            let Some(end) = rest.find(quote) else {
                return false;
            };
            let f = &rest[..end];
            flags = &rest[end + 1..];
            f
        } else {
            let end = flags.find([' ', '\t', '\r', '\n']).unwrap_or(flags.len());
            let f = &flags[..end];
            flags = &flags[end..];
            f
        };
        let name = field.split('=').next().unwrap_or(field);
        if name == "-tags" || name == "--tags" {
            return false;
        }
    }
}
