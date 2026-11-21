use pretty_assertions::assert_eq;

use super::execute;

fn s(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| (*a).to_string()).collect()
}

#[test]
fn argument_validation_and_exit_codes() {
    // Missing budget -> exit 2
    let out = execute(&s(&["echo", "hi"]));
    assert_eq!(out.exit_code, 2);
    assert!(out.stderr.starts_with("mrun: "));

    // Invalid --max-tokens -> exit 2
    for bad in [
        "--max-tokens=",
        "--max-tokens=0",
        "--max-tokens=01",
        "--max-tokens=abc",
    ] {
        let out = execute(&s(&[bad, "echo", "hi"]));
        assert_eq!(out.exit_code, 2);
        assert!(
            out.stderr
                .contains("--max-tokens requires one positive integer")
        );
    }

    // Repeated --head/--tail -> exit 2
    let out = execute(&s(&["-n", "2", "--head", "--tail", "echo", "hi"]));
    assert_eq!(out.exit_code, 2);
    assert!(out.stderr.contains("specify --head or --tail at most once"));

    // Missing command -> exit 127
    let out = execute(&s(&["-n", "2", "definitely_nonexistent_mekugi_cmd_xyz"]));
    assert_eq!(out.exit_code, 127);

    // Child options after command name are passed untouched
    let out = execute(&s(&["-n", "2", "printf", "%s\n", "--max-tokens=99"]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "--max-tokens=99\n");
    assert_eq!(out.stderr, "");
}

#[test]
fn line_mode_head_and_tail_discard() {
    let script = "printf 'L1\\nL2\\nL3\\nL4\\nL5\\n'";

    // Default is --tail
    let out = execute(&s(&["-n", "2", "sh", "-c", script]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "L4\nL5\n");
    assert!(!out.stdout.contains("L1"));
    assert_eq!(
        out.stderr,
        "mrun: output incomplete: 2-line limit reached\n"
    );

    // --head keeps first 2 lines and discards the rest
    let out = execute(&s(&["-n", "2", "--head", "sh", "-c", script]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "L1\nL2\n");
    assert!(!out.stdout.contains("L5"));
    assert_eq!(
        out.stderr,
        "mrun: output incomplete: 2-line limit reached\n"
    );

    // Within limit -> no omission notice
    let out = execute(&s(&["-n", "10", "sh", "-c", script]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "L1\nL2\nL3\nL4\nL5\n");
    assert_eq!(out.stderr, "");
}

#[test]
fn token_mode_head_tail_and_stderr_half_cap() {
    // 10 tokens = 40 bytes total; when stdout is non-empty, stderr <= 5 tokens (20 bytes)
    let script = "printf '%s' 'OUT_0123456789_ABCDEFGHIJKLMNOPQRSTUVWXYZ'; printf '%s' 'ERR_0123456789_ABCDEFGHIJKLMNOPQRSTUVWXYZ' >&2";

    let out = execute(&s(&["--max-tokens", "10", "--head", "sh", "-c", script]));
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.starts_with("OUT_"));
    assert!(out.stdout.len() <= 20, "stdout len={}", out.stdout.len());
    assert!(out.stderr.starts_with("ERR_"));
    let stderr_content = out.stderr.lines().next().unwrap();
    assert!(
        stderr_content.len() <= 20,
        "stderr cap exceeded: {stderr_content:?}"
    );
    assert!(
        out.stderr
            .ends_with("mrun: output incomplete: 10-token limit reached\n")
    );

    // --tail keeps end of streams
    let out = execute(&s(&["--max-tokens=10", "--tail", "sh", "-c", script]));
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.ends_with("XYZ"), "got {:?}", out.stdout);
    let stderr_content = out.stderr.lines().next().unwrap();
    assert!(stderr_content.ends_with("XYZ"), "got {:?}", stderr_content);
    assert!(
        out.stderr
            .ends_with("mrun: output incomplete: 10-token limit reached\n")
    );

    // Combined -n and --max-tokens notice wording
    let out = execute(&s(&[
        "-n",
        "2",
        "--max-tokens",
        "5",
        "sh",
        "-c",
        "printf 'a\\nb\\nc\\n'",
    ]));
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "b\nc\n");
    assert_eq!(
        out.stderr,
        "mrun: output incomplete: 2-line limit or 5-token limit reached\n"
    );
}

#[test]
fn preserves_nonzero_exit_and_unix_signal() {
    let out = execute(&s(&["-n", "2", "sh", "-c", "echo fail; exit 42"]));
    assert_eq!(out.exit_code, 42);
    assert_eq!(out.stdout, "fail\n");
    assert_eq!(out.stderr, "");

    #[cfg(unix)]
    {
        let out = execute(&s(&["-n", "2", "sh", "-c", "kill -9 $$"]));
        assert_eq!(out.exit_code, 128 + 9);
    }
}
