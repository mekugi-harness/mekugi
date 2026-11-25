# Development

Build, test, diagnose, and profile Mekugi. Run checkout commands from the repository root.
See [Contributing](../../CONTRIBUTING.md) for the contributor entry point.

## Build and test

From a checkout with [source prerequisites](../../README.md#build-from-source):

```sh
just build
just test
just lint
just docs-check
```

For a fast path-scoped verification loop in feature branches or worktrees, run
`just affected` (defaults to diffing `origin/main...HEAD` plus uncommitted
changes; pass a base ref or `--dry-run` to inspect planned commands). It runs
`just webui-check`, scoped `just test`, and scoped `just test-rust` (or
`just test-native` on workspace-level Rust changes) only for touched areas, and
skips Cargo completely when `codex-rs/` is untouched.

Recipes suppress command echoes and compact tool output when RTK is enabled.
Go tests summarize successful packages and stream failure diagnostics; lint keeps
native diagnostics and exit status. Set `RTK=` for native Go package outcomes.

`just webui` builds the pinned Svelte application for embedding.
`just build-binaries` and the release workflow include it. Plain `just build`
builds and checks the Go workspace.
Browser build and check recipes use **Bun 1.4.4+** on `PATH` to install pinned
dependencies from `bun.lock` and run package scripts. Node.js remains required
for the Vite build and browser binding tests.
`just webui-check` runs browser binding tests, `bun check` for TypeScript,
`svelte-check` for Svelte script/template and CSS diagnostics, lint and formatting
checks. Focused tests use `node --test` with
the selected test file under
`mekugi-webui/app/src/lib/`.
`just webui-fmt` formats the browser sources with `bun fmt`, using the existing
`.oxfmtrc.json` configuration. The assets host returns 503 until assets are built.

After building assets, the opt-in browser fixtures exercise the actual UI,
connector and server with controlled execution hosts:

```sh
MEKUGI_TEST_WEBUI_BROWSER=1 just test \
  ./mekugi-tui/apps/mekugi/internal/launch \
  -run '^TestWebUI(BrowserMachines|SemanticBrowser)$'
```

They require Chrome and `agent-browser`; `MEKUGI_TEST_AGENT_BROWSER` may select an
absolute executable. They cover machine isolation, reconnection, semantic controls,
attached-input recovery, native continuation and replay. Live-provider behavior and
deployed TLS require separate checks.

Run the session metrics and responsive Features checks independently of the
file-details flow:

```sh
MEKUGI_TEST_WEBUI_BROWSER=1 just test \
  ./mekugi-tui/apps/mekugi/internal/launch -run '^TestWebUIMetricsBrowser$'
```

The build, test, lint, generation, and snapshot recipes load
`mekugi-tui/go.work`. For raw Go commands, run inside `mekugi-tui/` or set
`GOWORK="$PWD/mekugi-tui/go.work"` at the repository root. `just build-binaries`
writes the Go executables and native runtime to `bin/` without installing them. Run member tests from
that member with the workspace enabled. Local modules are released together and
resolved by `go.work`, not unpublished version requirements or local replacements.
See [workspace ownership](../contracts/composition.md).

The [module reference](../reference/modules.md) is generated from workspace
descriptors, binding declarations and composition. `just generate` refreshes
the reference and browser wire types. `just check-generated` checks both
without rewriting them; CI runs the same check.

Interpreter and query fixtures use Node.js/ripgrep; lint needs `golangci-lint`/`deadcode` and installs neither.
Pass package paths and `go test` flags to focus tests. Use `@workspace` to
include all workspace packages when passing flags; `-count=1` bypasses caching.

```sh
# Saved Diff: presentation, rendering and shared diff primitives
just test ./mekugi-tui/modules/changehistory/presentation \
  ./mekugi-tui/modules/changehistory/terminalview \
  ./mekugi-tui/libraries/diffrender
# Live preview: provisional projection, streaming and rendering
just test ./mekugi-tui/modules/livepreview/...
```

Launch retains host/session integration checks. Run its affected tests when
changing those boundaries. Focused tests do not replace the snapshot gate.
Go builds use `-trimpath` to reuse cached packages across worktrees and staged
checks; use raw Go commands when you need absolute source paths.
Use `just test-host` for the focused runtime checks with local mock providers;
see [host acceptance](testing-host.md).
`just test-ui-snapshots` checks fixtures; review before updating. See
[snapshot procedures](testing-snapshots.md).
Enable the repository's Git hooks from the main checkout once per clone:

```sh
git config --local core.hooksPath "$(git rev-parse --show-toplevel)/.githooks"
```

When Git checks out a new linked worktree, the hook seeds it with a copy of the main
checkout's Graphify output, if present. This is a snapshot, not a refresh, and it
does not replace existing output.

When documentation changes are staged, the gate checks local link targets and Markdown
heading anchors across the staged documentation. It excludes dependency, build and
test-fixture directories. Run `just docs-check` to check the working tree.
The gate checks affected UI snapshots against the staged source in a temporary checkout.
Go source changes select snapshot packages through transitive imports, including test and
platform-tagged imports. Snapshot baseline changes select their owning package; shared
renderer changes still check downstream rendering consumers. Other inputs, including build
configuration and resources without known ownership, retain the full snapshot gate.
It skips commits made while a rebase is in progress, including amended or reworded steps, so
run `just docs-check` on the rebased tip when documentation changed, and
`just test-ui-snapshots` when UI sources changed.
The absolute hook path also applies the gate to this clone's other worktrees. To test changes
to the hook in another worktree, use `git -c core.hooksPath="$PWD/.githooks" commit` there;
this selects that worktree's hook without changing the clone's configuration.
Checkouts predating the Just migration need their own checkout's hook.
Unstaged fixes cannot hide stale staged fixtures. A mismatch blocks the commit and
prints its diff. Review and update the affected baselines, then stage them with the
source change. The existing Linux CI suite also checks all UI snapshots.
The [release workflow](../../.github/workflows/release.yml) packages Linux/macOS; tags publish assets.

## Preview the terminal UI

Build the native runtime with `just build-native`, then run
`just preview-native-ui` in a terminal. The preview runs real Mekugi against a
scripted local provider. It needs no credentials and calls no live model. Its
workspace and session data are discarded when it exits.

- Submit `demo` to watch a streamed edit and an agent review. Use **Ctrl+B, 2**
  to open Saved Diff and **Ctrl+B, 3** to inspect the agent.
- Submit `questions` to try the synchronous and asynchronous question flows.
  Use **Ctrl+B, Q** to open pending questions.
- Use `/quit` to exit.

## Native app-server

Build the [native runtime](../../codex-rs) from the same checkout, without
installing anything:

```sh
just build-native
just check-native
just test-native
just package-native
```

Building needs Python 3.11+, Rustup, a C/C++ toolchain and, on Linux,
`pkg-config`, `libcap-dev` and `libssl-dev`. Rustup selects the fork's pinned
toolchain; the build uses its pinned V8 downloader. First builds need network
access. Checks and tests default to `dev-small`. `just build-native` uses the
`release` profile with LTO disabled by default to reduce peak build memory.
Use `just build-native --profile release` to use the release profile's
ThinLTO setting. `just package-native` and CI release packaging use that setting
by default. CI runs native integration tests in `dev-small` and smoke-tests the
full release archive through the installer on each platform. To cap local
compile concurrency, optionally set
`CARGO_BUILD_JOBS=2`; the native runner does not set a jobs limit.

For an affected crate, pass Cargo selectors after `--`:

```sh
just check-native -- -p codex-core --tests
just test-native -- -p codex-core --lib -- capture_delivery
```

To run matching app-server integration cases with the freshly staged server and
code-mode host, use a test name or substring instead of Cargo selectors. The
filter applies to both the subprocess and broader app-server integration suites:

```sh
just test-native --test-filter suite::v2::code_mode_host
```

Use `check-native` to catch compilation errors without generating or linking the
application and test binaries.
Focused Cargo selectors skip separate runtime staging and packaging. A test-name
filter limits execution, not compilation of the selected test binary. Keep the same
profile and target directory across runs; Cargo retains separate check and test
artifacts.

In the `dev-small` profile, `codex-core` and `codex-git-utils` use rustc's
incremental compilation. Their first builds seed local artifacts; later edits
can reuse them. Other crates remain eligible for sccache. If your global Cargo
config sets `build.incremental = false`, scope that default to `[profile.dev]`
instead so it does not suppress the package settings. With sccache as the
compiler wrapper, set `SCCACHE_ALLOW_INCREMENTAL=1` so incremental builds
bypass the cache.

When staging the runtime for integration tests, `test-native` builds the runtime
binaries and selected suites together. By default it does not build the broader
app-server integration suite. The default subprocess harness does not link the
app-server library, so an app-server-only source edit does not rebuild it.
Both commands use the pinned V8 environment; tests inherit the fork's 8 MiB test
stack default. No cache cleanup is needed between runs.

The native runner honors Cargo's configured target directory, including global
`build.target-dir` and `CARGO_TARGET_DIR` overrides. V8 downloads and Cargo build
outputs use that same directory.
The runner forwards `CARGO_TARGET_DIR` and `CARGO_INCREMENTAL=0`/`1` as Cargo
options rather than compiler environment variables, avoiding unnecessary sccache
key splits. Equivalent configured and environment settings can reuse the same
cache entries; genuinely different compilation settings still have separate keys.

Builds and the default native regression suite put executables in `bin/native`;
packaging puts the archive in `bin/mekugi-native.tar.gz`. Native recipes accept
`--output` and `--archive` to change these paths. `just build-binaries` places
the complete client and runtime together in `bin/`; `just install` installs both
in the Go binary directory. Keep the server beside
`codex-code-mode-host` and Linux `bwrap`. The archive also includes protocol
schemas, the bundled model catalog, licenses and the source revision. Shells and
`rg` remain host tools.
Release CI packages native Linux amd64/arm64 and macOS arm64 builds.

Run `bin/native/mekugi-app-server` for stdio JSON-RPC, or pass
`--listen unix:///absolute/path/server.sock` for a Unix-socket attachment.
The fork's existing configuration and transport options remain available through
`--help`. Terminal, browser and headless sessions use this executable, including
their Codex children, Side and One-off turns. Launch resolves it from the caller's
`PATH`, then beside the Mekugi executable. There is no stock-runtime fallback for
these sessions. Mekugi does not wrap stock CLI commands such as `exec` or `review`.
Use `mekugi codex headless --yolo` with a stdin prompt for noninteractive execution;
run Codex administrative commands directly.

`just test-native` runs standalone CLI, command execution, connection,
code-mode-host and strict-configuration tests using local fixtures, not live
providers. When importing upstream Codex changes, review security changes, rerun
these checks and compare protocol changes with client consumers before packaging.

## Integrate reviewed work

Main can batch ready operations through the [workspace helper](../../mekugi-tui/apps/mekugi/cmd/workspace/integrate.go),
without a receipt or acceptance-state store. Supply explicit commits in pick order
or one reviewed Mekugi merge ref, and check argv after `--`:

```sh
just integrate --commits "COMMIT1 COMMIT2" -- \
  just test ./mekugi-tui/apps/mekugi/cmd/workspace
just integrate --merge REVIEWED_MEKUGI_REF -- just test-native
```

Preparation does not infer acceptance or set statuses. After Main's QA,
reuse already-completed related-batch checks by explicitly
attesting their full HEAD and choosing the desired status:

```sh
just integrate --checked-head FULL_HEAD --accept \
  --items "MEKUGI-123 MEKUGI-124" --state "Pending QA"
just integrate --checked-head FULL_HEAD --accept \
  --items "MEKUGI-123 MEKUGI-124" --state Done --cleanup .worktrees/task-name
```

`--accept` is Main's explicit authorization of the requested status/cleanup, not
an inferred result of a passed command. Check argv can instead accompany these
steps in one call when QA and authorization are already settled. Main owns the
coverage claim: the helper neither selects tests nor verifies that a successful
command ran adequate tests. Checks run in the actual integration worktree,
including preserved unrelated dirty files. A checked-HEAD attestation is not an
immutable-clean-tree certificate. Its full hash must match current HEAD and
cannot accompany new picks. No fresh full suite is imposed for each item.

A staged index is refused before mutation; unrelated unstaged/untracked files
are not staged, stashed or discarded. Git cherry-pick refuses actual overlaps
and retains configured signing. Merges also refuse local untracked or ignored
paths that overlap the incoming tree. If a merge fails, inspect Git and finish or
abort the merge manually. Earlier writes are not rolled back, and checks and
finalization do not run after a merge failure.
Check or Plane failures stop the sequence before cleanup; all requested statuses
must succeed first. Earlier successful writes are not rolled back. Inspect an
uncertain Plane write before retrying.

Repeat `--cleanup PATH` for explicitly authorized source worktrees beneath this
checkout's `.worktrees`. Cleanup checks exact repository/worktree identity, source
dirt, `git cherry` patch equivalence and unmatched merges, because `git cherry`
omits merge-only changes. Ignored files also block cleanup, except fixed
generated outputs:

- Browser dependencies and bundles: `mekugi-webui/app/node_modules/` and
  `mekugi-webui/dist/app/`.
- Native outputs: `codex-rs/target/`, `bin/native/` and
  `bin/mekugi-native.tar.gz`.
- Local generated data: `graphify-out/`, `runtime/__pycache__/` and
  `scripts/codex_package/__pycache__/`.

These outputs are discarded with the worktree. Neighboring ignored files and
custom output paths remain protected, and output outside the worktree is
untouched. Cleanup removes only the checked worktree, then its branch.
A cleanup failure retains whatever has not yet been removed; status writes
remain, and a failed branch removal leaves the branch for
recovery. Serialize integration and source edits; this is sequencing, not a
transaction or concurrent-mutation guarantee. Commit gates, signing, pushes and
additional browser/live-provider QA remain unchanged.

## Diagnostics

`--debug` records future requests under `$XDG_STATE_HOME/mekugi/debug` (default
`~/.local/state/mekugi/debug`). **Bundles contain private content and are not
sanitized.** Inactive bundles expire after 14 days; copy elsewhere to keep one. Exit
prints a diagnosis command. Codex tracing uses uncapped private temporary storage;
shutdown removes it, forced kills may leave it. See [diagnostics](../../mekugi-tui/modules/diagnostics/SPEC.md).

Automatically started local servers also retain private stderr and start/exit
records under `$XDG_STATE_HOME/mekugi/server` (default
`~/.local/state/mekugi/server`), even without `--debug` and after the client exits.
Inspect `process.jsonl` for timestamps, PID, build and exit evidence; read
`stderr.previous.log` before `stderr.log` for the retained tail. Stderr is bounded
to 1 MiB per server. These files are unsanitized and expire after 14 inactive days.
See [diagnostics](../../mekugi-tui/modules/diagnostics/SPEC.md)
for collection limits.

### Inspect a session

For historical behavior, start with recorded calls/results; current code explains intended behavior.
`--thread ID` finds the thread's rollout under `$CODEX_HOME/sessions` or `archived_sessions`
(default `~/.codex`) by its exact metadata identity; `--session PATH` reads a rollout file directly.
Child sessions and inherited fork history are not independent user sessions.

Session reports include the thread's retained `instructions`: what Mekugi forwarded to the model,
including environment facts, Mekugi guidance, developer messages and tool descriptions, as
content revisions with the time, model and turn each began. They are recorded without `--debug`;
older or expired threads report them as `unavailable`. Revisions reference `content` by digest.

Add `--ax` to inspect the thread's recent retained request phases without a debug
bundle. `ax.request_timing` distinguishes local preparation, request send, first
provider output and completion. Missing phases, including unpairable tool-result
readiness, are unknown rather than zero. First output is observed transport data,
not a first-token measurement. This bounded window does not establish lifetime
coverage or attribute every gap to Mekugi; see
[transport diagnostics](../../mekugi-tui/modules/transportdiagnostics/SPEC.md).

Inspection runs no recorded operations. Rollout and debug-bundle reports omit private
text unless `--field` selects it; `--field instructions` selects only retained instruction text.
`--request-id ID --field all` drills into one debug request.
Failure lookup with `--failures [REF]` always includes complete private error text,
which may contain request content or credentials.

```sh
mekugi inspect-session --thread THREAD_ID
mekugi inspect-session --thread THREAD_ID --ax
mekugi inspect-session --thread THREAD_ID --field instructions --text-bytes 65536
mekugi inspect-session --debug-dir /path/to/debug --field diagnostic --limit 5 --text-bytes 512
```

Narrow debug reports by `--request-id` and paginate with `--offset`. Select JSON fields rather
than line-limiting minified output.

See native retained inspection (`mekugi inspect-session`) and [AX evidence](../../mekugi-tui/modules/agentmetrics/SPEC.md).

## Profile live sessions

For reported live CPU/RAM faults, identify the affected binary, retained history/store scale,
and lifecycle phase. Sample the actual process within the authorized isolation boundary, or
reproduce those dimensions in an isolated process. Fresh sessions cannot establish
coverage of missing retained process state.

`just mekugi-pprof` builds a separate optimized executable with symbols and the
`pprof` build tag, plus its sibling executor, without installing or replacing the
normal command. Launch arguments, worker dispatch and hooks retain their behavior.
The normal build starts no profiling listener.

Launch with `bin/mekugi-pprof`.
Each owning process binds its own OS-assigned IPv4 loopback port and prints
`mekugi-pprof: http://127.0.0.1:PORT/debug/pprof/` to stderr before UI handoff.
The native interactive client opens its listener before connecting to the server;
profile that URL for client rendering and the server URL for session execution.
The standalone server opens its listener before accepting sessions. Start a CPU
capture before connecting a client to include session startup and resume costs;
all server-owned sessions share that listener. Use its URL with `go tool pprof`.
Workers and hooks start no listener; process completion cancels active captures
and closes the profiling listener.

Standard routes expose CPU, heap, allocs, goroutine, block, mutex and execution
trace data. CPU and trace collection start only on request. Block sampling uses
a 1 ms blocked-time rate; mutex sampling records one in ten contention events.
Instrumentation may perturb performance. Profiles cover the Mekugi process,
including UI, routing and replay, not separate Codex, executor or worker processes.

The routes have no additional authentication: local callers can read command-line
arguments and other private process data. Keep captures private. Profiling is
independent of sanitized metrics and debug evidence.
