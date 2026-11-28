//! Native Mekugi helper CLIs (`mcat`, `mfinspect`, `msymbol`, `mrun`) dispatched
//! through `codex-arg0` from the pinned `mekugi-app-server` binary.

mod common;
mod mcat;
mod mchanges;
mod mfinspect;
mod mrun;
mod msymbol;
mod sourcekind;
mod structure;

use std::ffi::OsString;
use std::io::Write;

/// Structured command output returned by the testable entry points.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

fn parse_os_args<I>(args: I) -> Result<Vec<String>, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut out = Vec::new();
    for arg in args {
        match arg.into_string() {
            Ok(valid) => out.push(valid),
            Err(invalid) => {
                return Err(format!(
                    "argument is not valid UTF-8: {}",
                    invalid.to_string_lossy()
                ));
            }
        }
    }
    Ok(out)
}

fn cli_args() -> Result<Vec<String>, String> {
    parse_os_args(std::env::args_os().skip(1))
}

fn emit_output(tool: &str, output: ToolOutput) -> i32 {
    let mut exit_code = output.exit_code;
    if !output.stdout.is_empty() {
        let mut stdout = std::io::stdout().lock();
        if let Err(err) = stdout
            .write_all(output.stdout.as_bytes())
            .and_then(|()| stdout.flush())
        {
            let mut stderr = std::io::stderr().lock();
            let _ = writeln!(stderr, "{tool}: write stdout: {err}");
            let _ = stderr.flush();
            exit_code = 1;
        }
    }
    if !output.stderr.is_empty() {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(output.stderr.as_bytes());
        let _ = stderr.flush();
    }
    exit_code
}

fn emit_arg_error(tool: &str, err: &str, exit_code: i32) -> i32 {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{tool}: {err}");
    let _ = stderr.flush();
    exit_code
}

/// Execute `mcat` with process arguments, streaming stdout directly and returning its exit code.
pub fn run_mcat_main() -> i32 {
    let args = match cli_args() {
        Ok(args) => args,
        Err(err) => return emit_arg_error("mcat", &err, 1),
    };
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    let mut code = mcat::execute_to_writers(&args, &mut stdout, &mut stderr);
    if let Err(err) = stdout.flush() {
        let _ = writeln!(stderr, "mcat: write stdout: {err}");
        code = 1;
    }
    let _ = stderr.flush();
    code
}

/// Execute `mfinspect` with process arguments, writing stdout/stderr and returning its exit code.
pub fn run_mfinspect_main() -> i32 {
    let args = match cli_args() {
        Ok(args) => args,
        Err(err) => return emit_arg_error("mfinspect", &err, 1),
    };
    emit_output("mfinspect", mfinspect::execute(&args))
}

/// Execute `msymbol` with process arguments, writing stdout/stderr and returning its exit code.
pub fn run_msymbol_main() -> i32 {
    let args = match cli_args() {
        Ok(args) => args,
        Err(err) => return emit_arg_error("msymbol", &err, 1),
    };
    emit_output("msymbol", msymbol::execute_standalone(&args))
}

/// Execute `mrun` with process arguments, writing stdout/stderr and returning its exit code.
pub fn run_mrun_main() -> i32 {
    let args = match cli_args() {
        Ok(args) => args,
        Err(err) => return emit_arg_error("mrun", &err, 2),
    };
    emit_output("mrun", mrun::execute(&args))
}

/// Execute `mchanges` with process arguments, writing stdout/stderr and returning its exit code.
pub fn run_mchanges_main() -> i32 {
    let args = match cli_args() {
        Ok(args) => args,
        Err(err) => return emit_arg_error("mchanges", &err, 2),
    };
    emit_output("mchanges", mchanges::execute(&args))
}

#[cfg(all(test, unix))]
mod cli_arg_tests {
    use super::parse_os_args;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn rejects_non_utf8_arguments_without_lossy_substitution() {
        let bad = OsString::from_vec(vec![b'f', b'o', b'o', 0xff, b'.', b't', b'x', b't']);
        let err = parse_os_args([OsString::from("valid.txt"), bad])
            .expect_err("non-UTF-8 argument must be rejected");
        assert!(err.contains("not valid UTF-8"));
    }
}
