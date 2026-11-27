use std::path::Path;
use std::path::PathBuf;

use codex_protocol::protocol::FileChange;
use pretty_assertions::assert_eq;

use super::JsonCmdExtractor;
use super::ShellEditProjector;

fn project_chunks(
    cwd: &str,
    chunks: &[&str],
) -> Vec<std::collections::HashMap<PathBuf, FileChange>> {
    let mut proj = ShellEditProjector::new(Path::new(cwd));
    let mut snapshots = Vec::new();
    for chunk in chunks {
        proj.push(chunk);
        if proj.take_dirty() {
            let snap = proj.snapshot();
            if !snap.is_empty() {
                snapshots.push(snap);
            }
        }
    }
    proj.finish();
    if proj.take_dirty() {
        let snap = proj.snapshot();
        if !snap.is_empty() {
            snapshots.push(snap);
        }
    }
    snapshots
}

#[test]
fn streams_cat_heredoc_incrementally_before_eof() {
    let snaps = project_chunks(
        "/repo",
        &[
            "cat <<'EOF' > src/hello.rs\n",
            "fn main() {\n",
            "    println!(\"hi\");\n",
            "}\n",
            "EOF\n",
        ],
    );
    assert!(
        snaps.len() >= 3,
        "expected incremental snapshots, got {snaps:?}"
    );
    assert_eq!(
        snaps[0].get(Path::new("/repo/src/hello.rs")),
        Some(&FileChange::Add {
            content: "fn main() {\n".to_string(),
        })
    );
    let last = snaps.last().expect("final snapshot");
    assert_eq!(
        last.get(Path::new("/repo/src/hello.rs")),
        Some(&FileChange::Add {
            content: "fn main() {\n    println!(\"hi\");\n}\n".to_string(),
        })
    );
}

#[test]
fn streams_cat_heredoc_after_cd_and_multiple_files() {
    let snaps = project_chunks(
        "/repo",
        &[
            "cd pkg && cat <<'EOF' > a.txt\nfirst\nEOF\n",
            "tee b.txt <<'END'\nsecond\nEND\n",
        ],
    );
    let last = snaps.last().expect("final snapshot");
    assert_eq!(
        last.get(Path::new("/repo/pkg/a.txt")),
        Some(&FileChange::Add {
            content: "first\n".to_string(),
        })
    );
    assert_eq!(
        last.get(Path::new("/repo/pkg/b.txt")),
        Some(&FileChange::Add {
            content: "second\n".to_string(),
        })
    );
}

#[test]
fn rejects_non_streaming_shell_copies_and_redirects() {
    for cmd in [
        "cat < large.txt > out.txt\n",
        "cat large.txt > out.txt\n",
        "cp large.txt out.txt\n",
        "mv a.txt b.txt\n",
        "echo hello > out.txt\n",
        "cat <<'EOF' > $DYNAMIC\nhi\nEOF\n",
    ] {
        let snaps = project_chunks("/repo", &[cmd]);
        assert!(
            snaps.is_empty(),
            "expected no live stream for {cmd:?}, got {snaps:?}"
        );
    }
}

#[test]
fn streams_python_heredoc_read_replace_write_text() {
    let snaps = project_chunks(
        "/repo",
        &[
            "python3 - <<'PY'\n",
            "from pathlib import Path\n",
            "p = Path('app.py')\n",
            "text = p.read_text()\n",
            "text = text.replace(\n    'old_one\\nold_two',\n",
            "    'new_one\\nnew_two'\n)\n",
            "p.write_text(text)\n",
            "PY\n",
        ],
    );
    assert!(
        snaps.len() >= 2,
        "expected incremental snapshots, got {snaps:?}"
    );
    let first = snaps.first().expect("first snapshot");
    assert_eq!(
        first.get(Path::new("/repo/app.py")),
        Some(&FileChange::Update {
            unified_diff: "@@\n-old_one\n-old_two\n".to_string(),
            move_path: None,
        })
    );
    let last = snaps.last().expect("final snapshot");
    assert_eq!(
        last.get(Path::new("/repo/app.py")),
        Some(&FileChange::Update {
            unified_diff: "@@\n-old_one\n-old_two\n+new_one\n+new_two\n".to_string(),
            move_path: None,
        })
    );
}

#[test]
fn streams_python_open_read_replace_and_inline_minus_c() {
    let snaps = project_chunks(
        "/repo",
        &[
            "python3 -c \"with open('notes.txt', 'r') as f: s = f.read()\n",
            "s = s.replace('before', 'after')\n",
            "with open('notes.txt', 'w') as f: f.write(s)\"",
        ],
    );
    let last = snaps.last().expect("final snapshot");
    assert_eq!(
        last.get(Path::new("/repo/notes.txt")),
        Some(&FileChange::Update {
            unified_diff: "@@\n-before\n+after\n".to_string(),
            move_path: None,
        })
    );
}

#[test]
fn streams_python_literal_write_text_and_open_write() {
    let snaps = project_chunks(
        "/repo",
        &[
            "python3 - <<'PY'\n",
            "from pathlib import Path\n",
            "Path('created.txt').write_text('line1\n",
            "line2\n')\n",
            "PY\n",
        ],
    );
    assert!(
        snaps.len() >= 2,
        "expected streaming write_text snapshots, got {snaps:?}"
    );
    let last = snaps.last().expect("final snapshot");
    assert_eq!(
        last.get(Path::new("/repo/created.txt")),
        Some(&FileChange::Add {
            content: "line1\nline2\n".to_string(),
        })
    );
}

#[test]
fn rejects_non_streaming_python_copies() {
    for cmd in [
        "python3 -c \"import shutil; shutil.copy('a.txt', 'b.txt')\"",
        "python3 -c \"import shutil; shutil.copy2('a.txt', 'b.txt')\"",
        "python3 -c \"from pathlib import Path; Path('b.txt').write_text(Path('a.txt').read_text())\"",
        "python3 - <<'PY'\nimport shutil\nshutil.copyfile('a', 'b')\nPY\n",
    ] {
        let snaps = project_chunks("/repo", &[cmd]);
        assert!(
            snaps.is_empty(),
            "expected no live stream for {cmd:?}, got {snaps:?}"
        );
    }
}

#[test]
fn json_cmd_extractor_streams_across_chunk_boundaries() {
    let mut ext = JsonCmdExtractor::default();
    let mut cmd = String::new();
    let mut workdir = None;
    for chunk in [
        "{\"work",
        "dir\":\"/repo/sub\",\"cm",
        "d\":\"cat <<'EOF' > f.txt\\",
        "nhi \\\"there\\\" \\u0041\\nEOF\",\"tty\":false}",
    ] {
        let (c, w) = ext.push(chunk);
        cmd.push_str(&c);
        if w.is_some() {
            workdir = w;
        }
    }
    assert_eq!(workdir.as_deref(), Some("/repo/sub"));
    assert_eq!(cmd, "cat <<'EOF' > f.txt\nhi \"there\" A\nEOF");
}
