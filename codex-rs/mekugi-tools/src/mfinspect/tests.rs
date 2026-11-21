use std::fs;

use pretty_assertions::assert_eq;
use serde_json::Value;
use tempfile::TempDir;

use super::execute;

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

#[test]
fn typescript_compact_and_json_spans() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("sample.ts");
    let source = "import {first} from \"one\";\nimport {second} from \"two\";\nfunction visible() {\n  return 1;\n}\n\nclass Box {\n  method() {}\n}";
    fs::write(&file, source).unwrap();
    let path = file.to_string_lossy().into_owned();

    let out = execute(&s(&[&path]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stderr, "");
    assert_eq!(
        out.stdout,
        "1-2 import\n3-5 function visible\n7-9 class Box\n8-8 method Box.method\n"
    );

    let out = execute(&s(&["--json", &path]));
    assert_eq!(out.exit_code, 0);
    let parsed: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["truncated"], false);
    assert_eq!(parsed["truncation"], Value::Null);
    assert_eq!(parsed["data"]["kind"], "code");
    assert_eq!(parsed["data"]["language"], "typescript");
    assert_eq!(parsed["data"]["line_count"], 9);
    assert_eq!(parsed["data"]["parse_complete"], true);
    assert_eq!(parsed["data"]["outline"].as_array().unwrap().len(), 5);
    assert_eq!(parsed["data"]["outline"][4]["receiver"], "Box");
}

#[test]
fn go_python_js_tsx_and_rust_outlines() {
    let dir = TempDir::new().unwrap();

    // Go
    let go_file = dir.path().join("sample.go");
    fs::write(
        &go_file,
        "package sample\n\nimport (\n\t\"fmt\"\n\t\"os\"\n)\n\ntype Server struct{}\n\nconst Version = \"1\"\n\nvar Default = 1\n\nfunc (s *Server) Run() {\n\tfmt.Println(os.Args)\n}\n\nfunc Helper() {}\n",
    )
    .unwrap();
    let out = execute(&s(&[&go_file.to_string_lossy()]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout,
        "4-5 import\n8-8 type Server\n10-10 constant Version\n12-12 variable Default\n14-16 method *Server.Run\n18-18 function Helper\n"
    );

    // Python
    let py_file = dir.path().join("sample.py");
    fs::write(
        &py_file,
        "import os\nfrom sys import argv as args\n\nX = 1\n\nclass Worker:\n    def run(self):\n        return X\n\ndef helper():\n    pass\n",
    )
    .unwrap();
    let out = execute(&s(&[&py_file.to_string_lossy()]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout,
        "1-2 import\n4-4 variable X\n6-8 class Worker\n7-8 method Worker.run\n10-11 function helper\n"
    );

    // JS + TSX
    let js_file = dir.path().join("app.js");
    fs::write(
        &js_file,
        "import './side-effect';\nconst LIMIT = 10;\nexport function render() { return LIMIT; }\n",
    )
    .unwrap();
    let out = execute(&s(&[&js_file.to_string_lossy()]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout,
        "1-1 import\n2-2 constant LIMIT\n3-3 function render\n"
    );

    let tsx_file = dir.path().join("view.tsx");
    fs::write(
        &tsx_file,
        "export function View() {\n  return <div />;\n}\n",
    )
    .unwrap();
    let out = execute(&s(&[&tsx_file.to_string_lossy()]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "1-3 function View\n");

    // Rust
    let rs_file = dir.path().join("lib.rs");
    fs::write(
        &rs_file,
        "use std::fmt::{self, Display};\n\n#[derive(Debug)]\npub struct Item<T>(T);\n\nimpl<T> Item<T> {\n    pub fn get(&self) -> &T { &self.0 }\n}\n\npub trait Runner {\n    fn step(&self);\n}\n\nconst MAX: usize = 1;\n",
    )
    .unwrap();
    let out = execute(&s(&[&rs_file.to_string_lossy()]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout,
        "1-1 import\n3-4 type Item\n7-7 method Item.get\n10-12 type Runner\n11-11 method Runner.step\n14-14 constant MAX\n"
    );
}

#[test]
fn markdown_and_json_strict_and_recovery() {
    let dir = TempDir::new().unwrap();

    // Markdown with YAML frontmatter and fenced code block containing #
    let md_file = dir.path().join("doc.md");
    fs::write(
        &md_file,
        "---\ntitle: Guide\ntags:\n  - a\n---\n# Intro\n```sh\n# not a heading\n```\n## Details\n",
    )
    .unwrap();
    let out = execute(&s(&[&md_file.to_string_lossy()]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout,
        "2-2 frontmatter title\n3-3 frontmatter tags\n6-6 heading Intro\n10-10 heading Details\n"
    );

    // Strict JSON with ~ and / escaping in pointer
    let json_file = dir.path().join("config.json");
    fs::write(&json_file, "{\n  \"a/b~c\": [true, 42]\n}\n").unwrap();
    let out = execute(&s(&[&json_file.to_string_lossy()]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(
        out.stdout,
        "1-3 json / object\n2-2 json /a~1b~0c array\n2-2 json /a~1b~0c/0 boolean\n2-2 json /a~1b~0c/1 number\n"
    );

    // Broken JSON falls back to recovery and sets parse_complete=false
    let bad_json = dir.path().join("broken.json");
    fs::write(&bad_json, "{\n  \"ok\": 1,\n  \"bad\": [\n}\n").unwrap();
    let out = execute(&s(&["--json", &bad_json.to_string_lossy()]));
    assert_eq!(out.exit_code, 0);
    let parsed: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(parsed["data"]["parse_complete"], false);
    assert!(
        parsed["data"]["outline"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "parse_error")
    );
    assert!(
        parsed["data"]["outline"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["pointer"] == "/ok")
    );
}

#[test]
fn unsupported_multipath_partial_failure_and_rejected_flags() {
    let dir = TempDir::new().unwrap();
    let unsupported = dir.path().join("notes.txt");
    fs::write(&unsupported, [0xff, 0xfe]).unwrap(); // never read for unsupported extension!
    let go_file = dir.path().join("ok.go");
    fs::write(&go_file, "package p\nfunc F() {}\n").unwrap();
    let missing = dir.path().join("nope.go");

    let out = execute(&s(&["--json", &unsupported.to_string_lossy()]));
    assert_eq!(out.exit_code, 0);
    let parsed: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(parsed["data"]["kind"], "none");
    assert_eq!(parsed["data"]["language"], Value::Null);
    assert_eq!(parsed["data"]["line_count"], Value::Null);
    assert_eq!(parsed["data"]["size_bytes"], 2);
    assert_eq!(parsed["data"]["parse_complete"], true);

    // Multi-path with partial failure
    let out = execute(&s(&[
        &go_file.to_string_lossy(),
        &missing.to_string_lossy(),
    ]));
    assert_eq!(out.exit_code, 1);
    assert!(out.stdout.contains("2-2 function F"));
    assert!(out.stdout.contains("--- "));
    assert!(out.stderr.starts_with("mfinspect: "));
    assert!(out.stderr.contains("path does not exist"));

    // Rejected flags
    let out = execute(&s(&["--preview-bytes=10", &go_file.to_string_lossy()]));
    assert_eq!(out.exit_code, 1);
    assert!(
        out.stderr
            .contains("mfinspect: mfinspect does not accept source preview flags")
    );

    let out = execute(&s(&["--max-tokens", "100", &go_file.to_string_lossy()]));
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.starts_with("mfinspect: "));

    // -- allows option-like paths
    let out = execute(&s(&["--", "--max-tokens=0"]));
    assert_eq!(out.exit_code, 1);
    assert!(out.stderr.contains("path does not exist"));
}
