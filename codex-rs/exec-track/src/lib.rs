//! Native exec tracking and shell-write / `mktemp` observation for Mekugi.
//!
//! Instruments supported Bash and Linux `dash`-backed `sh` command lists inside
//! a single real shell without occupying fixed caller FDs (3/4/7/8) or holding
//! tracking FDs open across user commands or detached descendants.
//!
//! - [`segment`]: Fail-closed `tree-sitter-bash` AST span selection and eligibility gate.
//! - [`rewrite`]: Single-shell boundary and `mktemp` instrumentation.
//! - [`session`]: Native pipe-synchronized control/ack session and report collector.
//! - [`capture`]: Pre/post shell-write and `mktemp` file-tree capture into `codex-change-history`.

mod capture;
mod rewrite;
mod segment;
#[cfg(unix)]
mod session;

pub use capture::ShellWriteCapture;
pub use capture::extract_static_write_paths;
pub use capture::shell_write_claim_key;
pub use rewrite::RewriteOptions;
pub use rewrite::TrackingMode;
pub use rewrite::instrument_script;
pub use rewrite::rewrite_segment_boundaries;
pub use segment::SegmentSpan;
pub use segment::ShellDialect;
pub use segment::SplitOutcome;
pub use segment::has_shell_setup;
pub use segment::split_segments;
#[cfg(unix)]
pub use session::DrainRequest;
#[cfg(unix)]
pub use session::ExecTrackReport;
#[cfg(unix)]
pub use session::ExecTrackSession;
#[cfg(unix)]
pub use session::MktempCreation;
#[cfg(unix)]
pub use session::OutputStreamKind;
#[cfg(unix)]
pub use session::SegmentReport;
#[cfg(unix)]
pub use session::SegmentStatus;

#[cfg(test)]
#[path = "exec_track_tests.rs"]
mod tests;
