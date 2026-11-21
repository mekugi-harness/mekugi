//! Native Mekugi helper CLIs (`mcat`, `mfinspect`, `msymbol`, `mrun`) dispatched
//! through `codex-arg0` from the pinned `mekugi-app-server` binary.

mod common;
mod mcat;
mod mfinspect;
mod mrun;
mod msymbol;
mod sourcekind;
mod structure;

use std::io::Write;

/// Structured command output returned by the testable entry points.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

fn cli_args() -> Vec<String> {
    std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn emit_output(output: ToolOutput) -> i32 {
    if !output.stdout.is_empty() {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(output.stdout.as_bytes());
        let _ = stdout.flush();
    }
    if !output.stderr.is_empty() {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(output.stderr.as_bytes());
        let _ = stderr.flush();
    }
    output.exit_code
}

/// Execute `mcat` with process arguments, writing stdout/stderr and returning its exit code.
pub fn run_mcat_main() -> i32 {
    emit_output(mcat::execute(&cli_args()))
}

/// Execute `mfinspect` with process arguments, writing stdout/stderr and returning its exit code.
pub fn run_mfinspect_main() -> i32 {
    emit_output(mfinspect::execute(&cli_args()))
}

/// Execute `msymbol` with process arguments, writing stdout/stderr and returning its exit code.
pub fn run_msymbol_main() -> i32 {
    emit_output(msymbol::execute_standalone(&cli_args()))
}

/// Execute `mrun` with process arguments, writing stdout/stderr and returning its exit code.
pub fn run_mrun_main() -> i32 {
    emit_output(mrun::execute(&cli_args()))
}

/// Run `mcat` in-process and capture stdout, stderr, and exit code.
pub fn run_mcat(args: &[String]) -> ToolOutput {
    mcat::execute(args)
}

/// Run `mfinspect` in-process and capture stdout, stderr, and exit code.
pub fn run_mfinspect(args: &[String]) -> ToolOutput {
    mfinspect::execute(args)
}

/// Run `msymbol` in-process and capture stdout, stderr, and exit code.
pub fn run_msymbol(args: &[String]) -> ToolOutput {
    msymbol::execute(args, /*dedicated_process*/ false)
}

/// Run `mrun` in-process and capture stdout, stderr, and exit code.
pub fn run_mrun(args: &[String]) -> ToolOutput {
    mrun::execute(args)
}
