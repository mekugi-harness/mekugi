use std::ffi::OsString;
use std::fs;
use std::path::Path;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::execute;
use super::execute_with_path;
use super::lsp::can_infer_go_tags;
use super::lsp::missing_go_build_tag;

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn test_path_with(bin_dir: &Path) -> OsString {
    let mut paths = vec![bin_dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).unwrap()
}

#[test]
fn cli_parsing_and_validation() {
    let out = execute(&s(&[]), false);
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.contains("usage: msymbol"));

    let out = execute(&s(&["badmode", "a.go", "Foo"]), false);
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.contains("mode must be def or refs"));

    let out = execute(&s(&["def", "a.go", "Foo", "2"]), false);
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.contains("N requires an explicit LINE"));

    let out = execute(&s(&["--max-tokens", "100", "def", "a.go", "Foo"]), false);
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.contains("unknown option"));

    let out = execute(&s(&["--workspace", "", "def", "a.go", "Foo"]), false);
    assert_eq!(out.exit_code, 1);
    assert!(
        out.stderr
            .contains("--workspace requires one usable directory")
    );
}

#[test]
fn workspace_confinement_symlink_escape_and_selection() {
    let outside = TempDir::new().unwrap();
    let secret = outside.path().join("secret.go");
    fs::write(&secret, "package p\nfunc Secret() {}\n").unwrap();

    let ws = TempDir::new().unwrap();
    let sample = ws.path().join("sample.go");
    fs::write(
        &sample,
        "package p\nfunc Dup() {}\nfunc Use() { x := 1 + 1; _ = x }\n",
    )
    .unwrap();

    #[cfg(unix)]
    {
        let link = ws.path().join("escape.go");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let out = execute(
            &s(&[
                "--workspace",
                &ws.path().to_string_lossy(),
                "def",
                "escape.go",
                "Secret",
            ]),
            false,
        );
        assert_eq!(out.exit_code, 1);
        assert!(out.stderr.contains("outside the workspace"));
    }

    // Outside via ..
    let out = execute(
        &s(&[
            "--workspace",
            &ws.path().to_string_lossy(),
            "def",
            "../outside.go",
            "Foo",
        ]),
        false,
    );
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.contains("outside the workspace"));

    // Symbol selection hints: missing outline declaration suggests token line
    let out = execute(
        &s(&[
            "--workspace",
            &ws.path().to_string_lossy(),
            "def",
            "sample.go",
            "x",
        ]),
        false,
    );
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.contains("no outline declaration"));
    assert!(out.stderr.contains("3"));

    // Ambiguous token on line requires N
    let out = execute(
        &s(&[
            "--workspace",
            &ws.path().to_string_lossy(),
            "def",
            "sample.go:3",
            "x",
        ]),
        false,
    );
    assert_eq!(out.exit_code, 1);
    assert!(
        out.stderr
            .contains("ambiguous on the selected line (2 occurrences); supply N")
    );

    // Go keyword rejected
    let out = execute(
        &s(&[
            "--workspace",
            &ws.path().to_string_lossy(),
            "def",
            "sample.go",
            "2",
            "func",
        ]),
        false,
    );
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.contains("non-keyword Go identifier"));
}

#[test]
fn go_build_tag_helpers() {
    assert_eq!(
        missing_go_build_tag("//go:build integration\npackage p\n"),
        Some("integration".to_string())
    );
    assert_eq!(missing_go_build_tag("//go:build a && b\npackage p\n"), None);
    assert_eq!(missing_go_build_tag("package p\n//go:build late\n"), None);
    assert!(can_infer_go_tags(""));
    assert!(can_infer_go_tags("-mod=readonly"));
    assert!(!can_infer_go_tags("-tags=foo"));
    assert!(!can_infer_go_tags("'-tags=foo'"));
    assert!(!can_infer_go_tags("--tags bar"));
}

#[cfg(unix)]
#[test]
fn fake_lsp_def_single_refs_batched_shared_process_and_source_change() {
    let ws = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    let counter_file = ws.path().join("spawn_count.txt");
    let ts_file = ws.path().join("app.ts");
    let ts_content = "export function alpha() {\n  return 1;\n}\nexport function beta() {\n  return alpha();\n}\n";
    fs::write(&ts_file, ts_content).unwrap();

    let canonical_ts = fs::canonicalize(&ts_file).unwrap();
    let ts_uri = super::symbol_file_uri(&canonical_ts);

    let fake_tsc = format!(
        r#"#!/usr/bin/env python3
import json, os, sys

with open({counter:?}, "a") as f:
    f.write("1\n")

def read_msg():
    headers = {{}}
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode().strip()
        if line == "":
            break
        k, v = line.split(":", 1)
        headers[k.lower()] = v.strip()
    n = int(headers["content-length"])
    return json.loads(sys.stdin.buffer.read(n).decode())

def send_msg(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(f"Content-Length: {{len(data)}}\r\n\r\n".encode() + data)
    sys.stdout.buffer.flush()

ts_uri = {ts_uri:?}
while True:
    msg = read_msg()
    if msg is None:
        break
    method = msg.get("method")
    if method == "initialize":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": {{"capabilities": {{"positionEncoding": "utf-16"}}}}}})
    elif method == "textDocument/definition":
        # Point to 'alpha' on line 0, cols 16..21 -> expands to lines 1-3!
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": [{{"uri": ts_uri, "range": {{"start": {{"line": 0, "character": 16}}, "end": {{"line": 0, "character": 21}}}}}}]}})
    elif method == "textDocument/references":
        pos = msg["params"]["position"]
        if pos["line"] == 0:
            locs = [
                {{"uri": ts_uri, "range": {{"start": {{"line": 0, "character": 16}}, "end": {{"line": 0, "character": 21}}}}}},
                {{"uri": ts_uri, "range": {{"start": {{"line": 4, "character": 9}}, "end": {{"line": 4, "character": 14}}}}}},
            ]
        else:
            locs = [
                {{"uri": ts_uri, "range": {{"start": {{"line": 3, "character": 16}}, "end": {{"line": 3, "character": 20}}}}}},
                {{"uri": ts_uri, "range": {{"start": {{"line": 4, "character": 9}}, "end": {{"line": 4, "character": 14}}}}}},
            ]
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": locs}})
    elif method == "shutdown":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": None}})
    elif method == "exit":
        break
"#,
        counter = counter_file.to_string_lossy(),
        ts_uri = ts_uri,
    );
    write_executable(&bin.path().join("tsc"), &fake_tsc);
    let path_env = test_path_with(bin.path());
    let ws_str = ws.path().to_string_lossy().into_owned();

    // 1. def expands to full function declaration lines 1-3
    let out = execute_with_path(
        &s(&["--workspace", &ws_str, "def", "app.ts", "5", "alpha"]),
        false,
        Some(&path_env),
    );
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(
        out.stdout
            .contains(":1-3\nexport function alpha() {\n  return 1;\n}\n")
    );

    // 2. single refs
    let out = execute_with_path(
        &s(&["--workspace", &ws_str, "refs", "app.ts", "alpha"]),
        false,
        Some(&path_env),
    );
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("1 export function alpha() {\n"));
    assert!(out.stdout.contains("5   return alpha();\n"));

    // 3. batched multi-tuple refs: verify ONE shared LSP process!
    fs::write(&counter_file, "").unwrap();
    let out = execute_with_path(
        &s(&[
            "--workspace",
            &ws_str,
            "refs",
            "app.ts",
            "alpha",
            "refs",
            "app.ts",
            "beta",
        ]),
        false,
        Some(&path_env),
    );
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("refs[1] "));
    assert!(out.stdout.contains("refs[2] "));
    assert!(out.stdout.contains("5 [1,2]   return alpha();\n"));
    let spawns = fs::read_to_string(&counter_file).unwrap().lines().count();
    assert_eq!(
        spawns, 1,
        "batched queries must share ONE LSP server process"
    );

    // 4. post-resolution source-change detection
    let mutating_tsc = format!(
        r#"#!/usr/bin/env python3
import json, sys
def read_msg():
    headers = {{}}
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        line = line.decode().strip()
        if line == "": break
        k, v = line.split(":", 1)
        headers[k.lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers["content-length"])).decode())
def send_msg(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(f"Content-Length: {{len(data)}}\r\n\r\n".encode() + data)
    sys.stdout.buffer.flush()
while True:
    msg = read_msg()
    if msg is None: break
    m = msg.get("method")
    if m == "initialize":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": {{"capabilities": {{}}}}}})
    elif m == "textDocument/definition":
        with open({ts_file:?}, "a") as f:
            f.write("// mutated\n")
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": []}})
    elif m == "shutdown":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": None}})
    elif m == "exit":
        break
"#,
        ts_file = canonical_ts.to_string_lossy(),
    );
    write_executable(&bin.path().join("tsc"), &mutating_tsc);
    let out = execute_with_path(
        &s(&["--workspace", &ws_str, "def", "app.ts", "alpha"]),
        false,
        Some(&path_env),
    );
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.contains("input changed during query"));
}

#[cfg(unix)]
#[test]
fn fake_gopls_build_tag_retry() {
    let ws = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    let go_file = ws.path().join("tagged.go");
    fs::write(
        &go_file,
        "//go:build customtag\n\npackage p\n\nfunc Tagged() {}\n",
    )
    .unwrap();
    let canonical_go = fs::canonicalize(&go_file).unwrap();
    let go_uri = super::symbol_file_uri(&canonical_go);

    write_executable(&bin.path().join("go"), "#!/bin/sh\necho ''\n");
    let fake_gopls = format!(
        r#"#!/usr/bin/env python3
import json, sys
def read_msg():
    headers = {{}}
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        line = line.decode().strip()
        if line == "": break
        k, v = line.split(":", 1)
        headers[k.lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers["content-length"])).decode())
def send_msg(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(f"Content-Length: {{len(data)}}\r\n\r\n".encode() + data)
    sys.stdout.buffer.flush()
go_uri = {go_uri:?}
has_tag = False
next_srv_id = 100
pending_def = None
while True:
    msg = read_msg()
    if msg is None: break
    m = msg.get("method")
    if m == "initialize":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": {{"capabilities": {{}}}}}})
    elif m == "workspace/didChangeConfiguration":
        next_srv_id += 1
        send_msg({{"jsonrpc": "2.0", "id": next_srv_id, "method": "workspace/configuration", "params": {{"items": [{{"section": "gopls"}}]}}}})
    elif "result" in msg and msg.get("id") == next_srv_id:
        cfg = msg["result"][0]
        if "-tags=customtag" in cfg.get("buildFlags", []):
            has_tag = True
        if pending_def is not None:
            send_msg({{"jsonrpc": "2.0", "id": pending_def, "result": [{{"uri": go_uri, "range": {{"start": {{"line": 4, "character": 5}}, "end": {{"line": 4, "character": 11}}}}}}]}})
            pending_def = None
    elif m == "textDocument/definition":
        if not has_tag and next_srv_id > 100:
            pending_def = msg["id"]
        elif not has_tag:
            send_msg({{"jsonrpc": "2.0", "id": msg["id"], "error": {{"code": -32603, "message": "no package metadata for file"}}}})
        else:
            send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": [{{"uri": go_uri, "range": {{"start": {{"line": 4, "character": 5}}, "end": {{"line": 4, "character": 11}}}}}}]}})
    elif m == "shutdown":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": None}})
    elif m == "exit":
        break
"#,
        go_uri = go_uri,
    );
    write_executable(&bin.path().join("gopls"), &fake_gopls);
    let path_env = test_path_with(bin.path());
    let out = execute_with_path(
        &s(&[
            "--workspace",
            &ws.path().to_string_lossy(),
            "def",
            "tagged.go",
            "Tagged",
        ]),
        false,
        Some(&path_env),
    );
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("func Tagged() {}"));
}

#[cfg(unix)]
#[test]
fn rust_preflight_non_cargo_and_proc_macro_incomplete_coverage() {
    // 1. Standalone non-Cargo .rs rejected before launching rust-analyzer
    let standalone_ws = TempDir::new().unwrap();
    let standalone_rs = standalone_ws.path().join("solo.rs");
    fs::write(&standalone_rs, "pub fn solo() {}\n").unwrap();
    let out = execute(
        &s(&[
            "--workspace",
            &standalone_ws.path().to_string_lossy(),
            "def",
            "solo.rs",
            "solo",
        ]),
        false,
    );
    assert_eq!(out.exit_code, 1);
    assert!(
        out.stderr
            .contains("standalone Rust files and non-Cargo projects are unsupported")
    );

    // 2. Cargo workspace with build-script -> RUSTUP stub verified, incomplete_coverage nonzero
    // while preserving available rows!
    let ws = TempDir::new().unwrap();
    let bin = TempDir::new().unwrap();
    let fake_sysroot = TempDir::new().unwrap();
    let core_lib = fake_sysroot
        .path()
        .join("lib/rustlib/src/rust/library/core/src/lib.rs");
    fs::create_dir_all(core_lib.parent().unwrap()).unwrap();
    fs::write(&core_lib, "").unwrap();

    fs::create_dir_all(ws.path().join("src")).unwrap();
    let manifest = ws.path().join("Cargo.toml");
    let lib_rs = ws.path().join("src/lib.rs");
    fs::write(
        &manifest,
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(&lib_rs, "pub fn hello() {}\n").unwrap();

    let canonical_ws = fs::canonicalize(ws.path()).unwrap();
    let canonical_manifest = fs::canonicalize(&manifest).unwrap();
    let canonical_lib = fs::canonicalize(&lib_rs).unwrap();
    let lib_uri = super::symbol_file_uri(&canonical_lib);
    let rustup_log = ws.path().join("rustup_check.txt");

    let metadata_json = serde_json::json!({
        "workspace_root": canonical_ws.to_string_lossy(),
        "workspace_members": ["demo 0.1.0"],
        "target_directory": canonical_ws.join("target").to_string_lossy(),
        "resolve": {"nodes": [{"id": "demo 0.1.0", "deps": []}]},
        "packages": [{
            "id": "demo 0.1.0",
            "manifest_path": canonical_manifest.to_string_lossy(),
            "source": null,
            "targets": [
                {"kind": ["lib"], "src_path": canonical_lib.to_string_lossy()},
                {"kind": ["custom-build"], "src_path": canonical_ws.join("build.rs").to_string_lossy()}
            ]
        }]
    })
    .to_string();

    let fake_cargo = format!(
        r#"#!/bin/sh
case "$1" in
  locate-project)
    printf '%s\n' {manifest:?}
    ;;
  -Z)
    printf '{{}}\n'
    ;;
  metadata)
    printf '%s\n' {metadata:?}
    ;;
  *)
    exit 1
    ;;
esac
"#,
        manifest = canonical_manifest.to_string_lossy(),
        metadata = metadata_json,
    );
    let fake_rustc = format!(
        r#"#!/bin/sh
if [ "$1" = "--print" ] && [ "$2" = "sysroot" ]; then
  printf '%s\n' {sysroot:?}
elif [ "$1" = "-vV" ]; then
  printf 'rustc 1.85.0\nhost: x86_64-unknown-linux-gnu\n'
else
  exit 1
fi
"#,
        sysroot = fake_sysroot.path().to_string_lossy(),
    );
    let fake_ra = format!(
        r#"#!/usr/bin/env python3
import json, os, subprocess, sys
if len(sys.argv) > 1 and sys.argv[1] == "--version":
    print("rust-analyzer 0.0.1")
    sys.exit(0)

# Verify RUSTUP stub exists and blocks installation with exit 1
rustup_bin = os.environ.get("RUSTUP", "")
rc = subprocess.run([rustup_bin, "component", "add", "rust-src"], capture_output=True, text=True)
with open({rustup_log:?}, "w") as f:
    f.write(f"rc={{rc.returncode}} stderr={{rc.stderr.strip()}} offline={{os.environ.get('CARGO_NET_OFFLINE')}} auto={{os.environ.get('RUSTUP_AUTO_INSTALL')}}")

def read_msg():
    headers = {{}}
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        line = line.decode().strip()
        if line == "": break
        k, v = line.split(":", 1)
        headers[k.lower()] = v.strip()
    return json.loads(sys.stdin.buffer.read(int(headers["content-length"])).decode())

def send_msg(obj):
    data = json.dumps(obj).encode()
    sys.stdout.buffer.write(f"Content-Length: {{len(data)}}\r\n\r\n".encode() + data)
    sys.stdout.buffer.flush()

lib_uri = {lib_uri:?}
while True:
    msg = read_msg()
    if msg is None: break
    m = msg.get("method")
    if m == "initialize":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": {{"capabilities": {{"positionEncoding": "utf-16", "diagnosticProvider": True}}}}}})
        send_msg({{"jsonrpc": "2.0", "method": "experimental/serverStatus", "params": {{"health": "ok", "quiescent": True, "message": ""}}}})
    elif m == "textDocument/diagnostic":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": {{"kind": "full", "items": []}}}})
    elif m == "textDocument/definition":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": [{{"uri": lib_uri, "range": {{"start": {{"line": 0, "character": 7}}, "end": {{"line": 0, "character": 12}}}}}}]}})
    elif m == "textDocument/hover":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": {{"contents": "fn hello()"}}}})
    elif m == "shutdown":
        send_msg({{"jsonrpc": "2.0", "id": msg["id"], "result": None}})
    elif m == "exit":
        break
"#,
        rustup_log = rustup_log.to_string_lossy(),
        lib_uri = lib_uri,
    );

    write_executable(&bin.path().join("cargo"), &fake_cargo);
    write_executable(&bin.path().join("rustc"), &fake_rustc);
    write_executable(&bin.path().join("rust-analyzer"), &fake_ra);
    let path_env = test_path_with(bin.path());

    let out = execute_with_path(
        &s(&[
            "--workspace",
            &canonical_ws.to_string_lossy(),
            "def",
            "src/lib.rs",
            "hello",
        ]),
        false,
        Some(&path_env),
    );
    assert_eq!(out.exit_code, 1);
    assert!(
        out.stdout.contains("pub fn hello() {}"),
        "available rows must still be preserved: {:?}",
        out.stdout
    );
    assert!(
        out.stderr
            .contains("disabled build scripts or procedural macros")
    );
    let log = fs::read_to_string(&rustup_log).unwrap();
    assert!(log.contains("rc=1"), "rustup stub must block: {log}");
    assert!(log.contains("offline=true"));
    assert!(log.contains("auto=0"));
}
