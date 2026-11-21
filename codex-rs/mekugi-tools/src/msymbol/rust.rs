use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use super::RETAINED_BYTES;
use super::SymbolQuery;
use super::lsp::LspClient;
use super::lsp::RpcEvent;
use super::lsp::RustDiagnosticReport;
use super::lsp::configure_process_group;
use super::lsp::resolve_binary;
use super::lsp::resolver_start_error;
use super::lsp::retire_child;
use crate::common::open_nonblocking;
use crate::common::outside_workspace;
use crate::structure::Position;

pub(crate) fn rust_coverage_msg(message: &str) -> String {
    format!("Rust coverage incomplete: {message}")
}

pub(crate) struct RustAnalysis {
    pub config: Value,
    pub sources: HashMap<usize, Vec<PathBuf>>,
    pub rustup: PathBuf,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    workspace_root: String,
    #[serde(default)]
    workspace_members: Vec<String>,
    #[serde(default)]
    target_directory: String,
    #[serde(default)]
    resolve: Option<CargoResolve>,
    #[serde(default)]
    packages: Vec<CargoPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoResolve {
    #[serde(default)]
    nodes: Vec<CargoResolveNode>,
}

#[derive(Debug, Deserialize)]
struct CargoResolveNode {
    id: String,
    #[serde(default)]
    deps: Vec<CargoResolveDep>,
}

#[derive(Debug, Deserialize)]
struct CargoResolveDep {
    pkg: String,
}

#[derive(Debug, Deserialize)]
struct CargoPackage {
    id: String,
    manifest_path: String,
    source: Option<String>,
    #[serde(default)]
    targets: Vec<CargoTarget>,
}

#[derive(Debug, Deserialize)]
struct CargoTarget {
    #[serde(default)]
    kind: Vec<String>,
    #[serde(default)]
    src_path: String,
}

fn run_prereq(
    deadline: Instant,
    dedicated_process: bool,
    dir: &Path,
    scratch: &Path,
    path_override: Option<&std::ffi::OsStr>,
    name: &str,
    args: &[&str],
) -> Result<Vec<u8>, String> {
    if Instant::now() >= deadline {
        return Err("deadline_exceeded".to_string());
    }
    let mut cmd = Command::new(resolve_binary(name, path_override));
    cmd.args(args)
        .current_dir(dir)
        .env("CARGO_NET_OFFLINE", "true")
        .env("RUSTUP_AUTO_INSTALL", "0")
        .env("TMPDIR", scratch)
        .env("TMP", scratch)
        .env("TEMP", scratch)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(p) = path_override {
        cmd.env("PATH", p);
    }
    if args.first().copied() == Some("-Z") {
        cmd.env("RUSTC_BOOTSTRAP", "1");
    }
    configure_process_group(&mut cmd, dedicated_process);

    let mut child = cmd.spawn().map_err(|err| {
        format!(
            "Rust prerequisite {name}: {}: ",
            resolver_start_error(name, &err)
        )
    })?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let out_handle = std::thread::spawn(move || read_bounded(stdout_pipe, RETAINED_BYTES + 1));
    let err_handle = std::thread::spawn(move || read_bounded(stderr_pipe, 8192));

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                retire_child(&mut child, dedicated_process);
                break None;
            }
        }
    };
    retire_child(&mut child, dedicated_process);

    let out_bytes = out_handle.join().unwrap_or_default();
    let err_bytes = err_handle.join().unwrap_or_default();

    let Some(status) = status else {
        return Err("deadline_exceeded".to_string());
    };
    if !status.success() {
        let diag = String::from_utf8_lossy(&err_bytes);
        return Err(format!(
            "Rust prerequisite {name}: exit status {status}: {diag}"
        ));
    }
    if out_bytes.len() > RETAINED_BYTES {
        return Err(rust_coverage_msg("prerequisite output exceeds 16 MiB"));
    }
    Ok(out_bytes)
}

fn read_bounded<R: Read>(pipe: Option<R>, limit: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let Some(mut pipe) = pipe else {
        return buf;
    };
    let mut chunk = [0u8; 8192];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let avail = limit.saturating_sub(buf.len());
                if avail > 0 {
                    buf.extend_from_slice(&chunk[..n.min(avail)]);
                }
            }
        }
    }
    buf
}

pub(crate) fn prepare_rust(
    deadline: Instant,
    dedicated_process: bool,
    root: &Path,
    scratch: &Path,
    queries: &mut [&mut SymbolQuery],
    path_override: Option<&std::ffi::OsStr>,
) -> Result<RustAnalysis, String> {
    let mut manifests: HashMap<PathBuf, PathBuf> = HashMap::new();
    let mut query_manifests: HashMap<usize, PathBuf> = HashMap::new();

    for (q_idx, q) in queries.iter_mut().enumerate() {
        let Some(file) = q.file.as_ref() else {
            continue;
        };
        let mut dir = file.path.parent().map(Path::to_path_buf);
        while let Some(cur) = dir {
            if outside_workspace(root, &cur) {
                break;
            }
            let candidate = cur.join("Cargo.toml");
            if candidate.metadata().is_ok_and(|m| m.is_file()) {
                let canonical = fs::canonicalize(&candidate)
                    .map_err(|_| rust_coverage_msg("Cargo manifest escapes workspace"))?;
                if outside_workspace(root, &canonical) {
                    return Err(rust_coverage_msg("Cargo manifest escapes workspace"));
                }
                manifests.entry(canonical.clone()).or_default();
                query_manifests.insert(q_idx, canonical);
                break;
            }
            if cur == root {
                break;
            }
            dir = cur.parent().map(Path::to_path_buf);
        }
        if !query_manifests.contains_key(&q_idx) {
            q.err = Some(rust_coverage_msg(
                "standalone Rust files and non-Cargo projects are unsupported",
            ));
        }
    }

    if manifests.is_empty() {
        return Err(rust_coverage_msg(
            "no Cargo project inside selected workspace",
        ));
    }

    let mut roots: BTreeSet<PathBuf> = BTreeSet::new();
    let manifest_keys: Vec<PathBuf> = manifests.keys().cloned().collect();
    for manifest in manifest_keys {
        let dir = manifest.parent().unwrap_or(root);
        let manifest_str = manifest.to_string_lossy();
        let located = run_prereq(
            deadline,
            dedicated_process,
            dir,
            scratch,
            path_override,
            "cargo",
            &[
                "locate-project",
                "--workspace",
                "--manifest-path",
                &manifest_str,
                "--message-format",
                "plain",
            ],
        )?;
        let located_str = String::from_utf8_lossy(&located).trim().to_string();
        let canonical = fs::canonicalize(&located_str).map_err(|_| {
            rust_coverage_msg("Cargo workspace manifest escapes selected workspace")
        })?;
        if outside_workspace(root, &canonical) {
            return Err(rust_coverage_msg(
                "Cargo workspace manifest escapes selected workspace",
            ));
        }
        manifests.insert(manifest, canonical.clone());
        roots.insert(canonical);
    }

    run_prereq(
        deadline,
        dedicated_process,
        root,
        scratch,
        path_override,
        "rust-analyzer",
        &["--version"],
    )?;

    // Write scratch RUSTUP stub blocking automatic toolchain/component installation.
    let rustup = scratch.join("rustup-no-install");
    let stub = "#!/bin/sh\nprintf '%s\\n' 'msymbol: automatic Rust component/toolchain installation is disabled' >&2\nexit 1\n";
    fs::write(&rustup, stub).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&rustup, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }

    let mut config_dirs: HashSet<PathBuf> = HashSet::new();
    let mut source_roots: HashMap<PathBuf, bool> = HashMap::new();
    let mut graph_roots: HashMap<PathBuf, HashMap<PathBuf, bool>> = HashMap::new();
    let mut exclusions: HashSet<PathBuf> = HashSet::new();
    let mut coverage: HashMap<PathBuf, String> = HashMap::new();
    let linked: Vec<PathBuf> = roots.into_iter().collect();

    for manifest in &linked {
        let directory = manifest.parent().unwrap_or(root);
        let sysroot_bytes = run_prereq(
            deadline,
            dedicated_process,
            directory,
            scratch,
            path_override,
            "rustc",
            &["--print", "sysroot"],
        )?;
        let sysroot = String::from_utf8_lossy(&sysroot_bytes).trim().to_string();
        let core_src = Path::new(&sysroot)
            .join("lib")
            .join("rustlib")
            .join("src")
            .join("rust")
            .join("library")
            .join("core")
            .join("src")
            .join("lib.rs");
        if !core_src.metadata().is_ok_and(|m| m.is_file()) {
            return Err(
                "Rust standard-library sources unavailable in selected toolchain; prepare rust-src separately (nothing installed)"
                    .to_string(),
            );
        }

        let config_raw = run_prereq(
            deadline,
            dedicated_process,
            directory,
            scratch,
            path_override,
            "cargo",
            &[
                "-Z",
                "unstable-options",
                "config",
                "get",
                "--format",
                "json",
            ],
        )?;
        let cargo_cfg: Value = serde_json::from_slice(&config_raw)
            .map_err(|err| format!("Rust Cargo configuration: {err}"))?;
        let mut platforms: Vec<String> = Vec::new();
        if let Some(target) = cargo_cfg.get("build").and_then(|b| b.get("target")) {
            match target {
                Value::String(s) => platforms.push(s.clone()),
                Value::Array(arr) => {
                    for item in arr {
                        let Some(s) = item.as_str() else {
                            return Err(rust_coverage_msg("invalid Cargo build.target"));
                        };
                        platforms.push(s.to_string());
                    }
                }
                Value::Null => {}
                _ => return Err(rust_coverage_msg("invalid Cargo build.target")),
            }
        }
        if platforms.is_empty() {
            let version_bytes = run_prereq(
                deadline,
                dedicated_process,
                directory,
                scratch,
                path_override,
                "rustc",
                &["-vV"],
            )?;
            let version_str = String::from_utf8_lossy(&version_bytes);
            for line in version_str.lines() {
                if let Some(host) = line.trim().strip_prefix("host: ") {
                    platforms.push(host.to_string());
                }
            }
            if platforms.is_empty() {
                return Err(rust_coverage_msg(
                    "rustc did not identify the selected host target",
                ));
            }
        }

        let manifest_str = manifest.to_string_lossy().into_owned();
        let mut meta_args: Vec<&str> = vec![
            "metadata",
            "--offline",
            "--locked",
            "--format-version",
            "1",
            "--manifest-path",
            &manifest_str,
        ];
        for p in &platforms {
            meta_args.push("--filter-platform");
            meta_args.push(p.as_str());
        }
        let raw_meta = run_prereq(
            deadline,
            dedicated_process,
            directory,
            scratch,
            path_override,
            "cargo",
            &meta_args,
        )?;
        let metadata: CargoMetadata = serde_json::from_slice(&raw_meta)
            .map_err(|err| format!("Rust Cargo metadata: {err}"))?;

        let canonical_ws = fs::canonicalize(&metadata.workspace_root)
            .map_err(|_| rust_coverage_msg("Cargo workspace root escapes selected workspace"))?;
        if outside_workspace(root, &canonical_ws) {
            return Err(rust_coverage_msg(
                "Cargo workspace root escapes selected workspace",
            ));
        }
        let resolve_nodes = metadata
            .resolve
            .as_ref()
            .map(|r| r.nodes.as_slice())
            .unwrap_or(&[]);
        if metadata.workspace_members.is_empty() || resolve_nodes.is_empty() {
            return Err(rust_coverage_msg(
                "Cargo metadata has no resolved workspace graph",
            ));
        }
        config_dirs.insert(canonical_ws);
        if !metadata.target_directory.is_empty() {
            exclusions.insert(PathBuf::from(&metadata.target_directory));
        }

        let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
        for node in resolve_nodes {
            let entry = edges.entry(node.id.as_str()).or_default();
            for dep in &node.deps {
                entry.push(dep.pkg.as_str());
            }
        }
        let mut active: HashSet<&str> = HashSet::new();
        let mut stack: Vec<&str> = metadata
            .workspace_members
            .iter()
            .map(String::as_str)
            .collect();
        while let Some(id) = stack.pop() {
            if active.insert(id)
                && let Some(deps) = edges.get(id)
            {
                stack.extend(deps.iter().copied());
            }
        }

        let member_set: HashSet<&str> = metadata
            .workspace_members
            .iter()
            .map(String::as_str)
            .collect();
        let entry_roots = graph_roots.entry(manifest.clone()).or_default();
        for pkg in &metadata.packages {
            let is_active = active.contains(pkg.id.as_str());
            if pkg.source.is_none() {
                let pkg_dir = Path::new(&pkg.manifest_path)
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default();
                config_dirs.insert(pkg_dir.clone());
                let cur = source_roots.entry(pkg_dir.clone()).or_insert(false);
                *cur = *cur || is_active;
                entry_roots.insert(pkg_dir.clone(), is_active);
                exclusions.insert(pkg_dir.join(".git"));
                exclusions.insert(pkg_dir.join("target"));
                if !member_set.contains(pkg.id.as_str()) {
                    for sub in ["tests", "examples", "benches"] {
                        exclusions.insert(pkg_dir.join(sub));
                    }
                }
                for target in &pkg.targets {
                    if !target.src_path.is_empty()
                        && let Some(t_dir) = Path::new(&target.src_path).parent()
                        && outside_workspace(&pkg_dir, t_dir)
                    {
                        let td = t_dir.to_path_buf();
                        let cur = source_roots.entry(td.clone()).or_insert(false);
                        *cur = *cur || is_active;
                        entry_roots.insert(td, is_active);
                    }
                }
            }
            for target in &pkg.targets {
                if is_active
                    && (target.kind.iter().any(|k| k == "custom-build")
                        || target.kind.iter().any(|k| k == "proc-macro"))
                {
                    coverage.insert(
                        manifest.clone(),
                        rust_coverage_msg(
                            "selected Cargo graph contains disabled build scripts or procedural macros; generated references are unavailable",
                        ),
                    );
                }
            }
        }
    }

    let linked_strs: Vec<String> = linked
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let rustup_str = rustup.to_string_lossy().into_owned();
    let config = json!({
        "cachePriming": {"enable": false},
        "linkedProjects": linked_strs,
        "cargo": {
            "buildScripts": {"enable": false, "rebuildOnSave": false, "overrideCommand": Value::Null},
            "extraArgs": ["--offline", "--locked"],
            "metadataExtraArgs": ["--offline", "--locked"],
            "extraEnv": {
                "CARGO_NET_OFFLINE": "true",
                "RUSTUP_AUTO_INSTALL": "0",
                "RUSTUP": rustup_str,
            },
            "sysroot": "discover",
            "sysrootSrc": Value::Null,
            "features": [],
            "noDefaultFeatures": false,
            "autoreload": true,
            "configPath": Value::Null,
            "noDeps": false,
            "target": Value::Null,
            "cfgs": [],
        },
        "procMacro": {"enable": false, "server": Value::Null},
        "checkOnSave": false,
        "check": {"overrideCommand": Value::Null},
        "workspace": {"discoverConfig": Value::Null},
        "rustc": {"source": Value::Null},
        "files": {"watcher": "client", "exclude": []},
        "diagnostics": {"enable": true, "disabled": []},
    });

    for dir in &config_dirs {
        for name in ["rust-analyzer.toml", ".rust-analyzer.toml"] {
            check_rust_config_file(&dir.join(name), &config)?;
        }
    }

    let scanned = scan_rust_sources(deadline, &source_roots, &exclusions, &config)?;
    let mut project_sources: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for (manifest, dirs) in graph_roots {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut list: Vec<PathBuf> = Vec::new();
        for (dir, active) in dirs {
            if !active {
                continue;
            }
            if let Some(files) = scanned.get(&dir) {
                for f in files {
                    if seen.insert(f.clone()) {
                        list.push(f.clone());
                    }
                }
            }
        }
        list.sort();
        project_sources.insert(manifest, list);
    }

    let mut sources_by_query: HashMap<usize, Vec<PathBuf>> = HashMap::new();
    for (q_idx, q) in queries.iter_mut().enumerate() {
        if let Some(local_manifest) = query_manifests.get(&q_idx)
            && let Some(ws_manifest) = manifests.get(local_manifest)
        {
            if let Some(cov) = coverage.get(ws_manifest) {
                q.coverage = Some(cov.clone());
            }
            if let Some(list) = project_sources.get(ws_manifest) {
                sources_by_query.insert(q_idx, list.clone());
            }
        }
    }

    Ok(RustAnalysis {
        config,
        sources: sources_by_query,
        rustup,
    })
}

fn scan_rust_sources(
    deadline: Instant,
    roots: &HashMap<PathBuf, bool>,
    exclusions: &HashSet<PathBuf>,
    config: &Value,
) -> Result<HashMap<PathBuf, Vec<PathBuf>>, String> {
    let mut checked_config: HashSet<PathBuf> = HashSet::new();
    let mut inventories: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();

    for (root, &active) in roots {
        let mut visited: HashSet<PathBuf> = HashSet::new();
        let mut sources: HashMap<PathBuf, PathBuf> = HashMap::new();
        walk_rust_dir(
            deadline,
            root,
            active,
            exclusions,
            config,
            &mut visited,
            &mut checked_config,
            &mut sources,
        )?;
        let mut list: Vec<PathBuf> = sources.into_values().collect();
        list.sort();
        inventories.insert(root.clone(), list);
    }
    Ok(inventories)
}

#[allow(clippy::too_many_arguments)]
fn walk_rust_dir(
    deadline: Instant,
    path: &Path,
    active: bool,
    exclusions: &HashSet<PathBuf>,
    config: &Value,
    visited: &mut HashSet<PathBuf>,
    checked_config: &mut HashSet<PathBuf>,
    sources: &mut HashMap<PathBuf, PathBuf>,
) -> Result<(), String> {
    if Instant::now() >= deadline {
        return Err("deadline_exceeded".to_string());
    }
    if exclusions.contains(path) {
        return Ok(());
    }
    let canonical = match fs::canonicalize(path) {
        Ok(c) => c,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.to_string()),
    };
    if exclusions.contains(&canonical) {
        return Ok(());
    }
    let meta = fs::metadata(&canonical).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        if !visited.insert(canonical.clone()) {
            return Ok(());
        }
        let entries = fs::read_dir(&canonical).map_err(|e| e.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            walk_rust_dir(
                deadline,
                &path.join(entry.file_name()),
                active,
                exclusions,
                config,
                visited,
                checked_config,
                sources,
            )?;
        }
        return Ok(());
    }

    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if (name == "rust-analyzer.toml" || name == ".rust-analyzer.toml")
        && checked_config.insert(canonical.clone())
    {
        check_rust_config_file(path, config)?;
    }
    if active && meta.is_file() && name.ends_with(".rs") {
        sources
            .entry(canonical)
            .or_insert_with(|| path.to_path_buf());
    }
    Ok(())
}

pub(crate) fn check_rust_config_file(path: &Path, safe_config: &Value) -> Result<(), String> {
    let mut file = match open_nonblocking(path) {
        Ok(f) => f,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.to_string()),
    };
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err(rust_coverage_msg(&format!(
            "configuration is not a regular file: {}",
            path.display()
        )));
    }
    let mut buf = Vec::new();
    let mut take = (&mut file).take((RETAINED_BYTES + 1) as u64);
    take.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    if buf.len() > RETAINED_BYTES {
        return Err(format!(
            "{}: Rust configuration must be UTF-8 and at most 16 MiB",
            path.display()
        ));
    }
    let text = String::from_utf8(buf).map_err(|_| {
        format!(
            "{}: Rust configuration must be UTF-8 and at most 16 MiB",
            path.display()
        )
    })?;
    let toml_val: toml::Value = toml::from_str(&text)
        .map_err(|err| format!("Rust configuration {}: {err}", path.display()))?;
    let json_val =
        serde_json::to_value(toml_val).map_err(|err| format!("Rust configuration: {err}"))?;
    if let Some(map) = json_val.as_object() {
        check_rust_configuration(map, safe_config.as_object(), "")
            .map_err(|err| format!("{}: {err}", path.display()))?;
    }
    Ok(())
}

fn check_rust_configuration(
    settings: &serde_json::Map<String, Value>,
    safe: Option<&serde_json::Map<String, Value>>,
    prefix: &str,
) -> Result<(), String> {
    const PROTECTED_ROOTS: &[&str] = &[
        "cargo",
        "procMacro",
        "checkOnSave",
        "check",
        "workspace",
        "rustc",
        "linkedProjects",
        "files",
        "diagnostics",
        "cfg",
    ];

    for (key, value) in settings {
        if prefix.is_empty() && !PROTECTED_ROOTS.contains(&key.as_str()) {
            continue;
        }
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        let expected = safe.and_then(|m| m.get(key));
        if let Some(child_map) = value.as_object() {
            check_rust_configuration(child_map, expected.and_then(Value::as_object), &path)?;
            continue;
        }
        if expected != Some(value) {
            return Err(rust_coverage_msg(&format!(
                "project configuration overrides protected setting {path}"
            )));
        }
    }
    Ok(())
}

pub(crate) fn rust_ready<W: Write>(
    lsp: &mut LspClient<W>,
    deadline: Instant,
) -> Result<(), String> {
    while lsp.rust_status.as_ref().is_none_or(|s| !s.quiescent) {
        match lsp.recv_event(deadline)? {
            RpcEvent::Eof => {
                return Err(rust_coverage_msg("language server exited before ready"));
            }
            RpcEvent::Error(err) => return Err(err),
            RpcEvent::Message(msg) => {
                lsp.handle_server_message(&msg)?;
            }
        }
    }
    if let Some(status) = &lsp.rust_status
        && status.health != "ok"
    {
        return Err(rust_coverage_msg(&format!(
            "rust-analyzer {}: {}",
            status.health, status.message
        )));
    }
    Ok(())
}

pub(crate) fn rust_diagnostics<W: Write>(
    lsp: &mut LspClient<W>,
    deadline: Instant,
    uri: &str,
    selection: Option<Position>,
) -> Result<(), String> {
    let report = if let Some(cached) = lsp.rust_reports.get(uri) {
        cached.clone()
    } else {
        let raw = lsp
            .request(
                deadline,
                "textDocument/diagnostic",
                Some(json!({"textDocument": {"uri": uri}})),
            )
            .map_err(|err| rust_coverage_msg(&format!("native diagnostics unavailable: {err}")))?;
        let parsed: RustDiagnosticReport = serde_json::from_value(raw)
            .map_err(|_| rust_coverage_msg("invalid native diagnostic report"))?;
        if parsed.kind != "full" {
            return Err(rust_coverage_msg("invalid native diagnostic report"));
        }
        lsp.rust_reports.insert(uri.to_string(), parsed.clone());
        parsed
    };
    check_rust_diagnostic_report(&report, selection)
}

fn format_diag_code(code: &Option<Value>) -> String {
    match code {
        None | Some(Value::Null) => "<nil>".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

fn check_rust_diagnostic_report(
    report: &RustDiagnosticReport,
    selected: Option<Position>,
) -> Result<(), String> {
    if report.kind != "full" {
        return Err(rust_coverage_msg(
            "invalid related native diagnostic report",
        ));
    }
    if selected.is_none()
        && report
            .items
            .iter()
            .any(|item| format_diag_code(&item.code) == "unlinked-file")
    {
        return Ok(());
    }
    for item in &report.items {
        let code = format_diag_code(&item.code);
        let mut inactive = false;
        if code == "inactive-code"
            && let Some(sel) = selected
        {
            let start = item.range.start;
            let end = item.range.end;
            let after_start = sel.line > start.line
                || (sel.line == start.line && sel.character >= start.character);
            let before_end =
                sel.line < end.line || (sel.line == end.line && sel.character < end.character);
            inactive = after_start && before_end;
        }
        if item.severity == 1
            || inactive
            || (selected.is_some() && code == "unlinked-file")
            || code.contains("unresolved")
        {
            return Err(rust_coverage_msg(&format!("{code}: {}", item.message)));
        }
    }
    for (related, maybe_report) in &report.related_documents {
        if let Some(sub) = maybe_report {
            check_rust_diagnostic_report(sub, None).map_err(|err| format!("{related}: {err}"))?;
        }
    }
    Ok(())
}
