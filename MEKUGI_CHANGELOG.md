# Mekugi native fork changelog

This is the current inventory of intentional differences from stock Codex.
It covers the native runtime Mekugi ships, not the host UI or changes already
in upstream Codex.

The fork started at `rust-v0.169.0`. The current comparison baseline is
`rust-v0.172.0` (imported atop PR #10). Upstream imports are squashes.
Compare the release tree directly with the fork tree, not their Git merge base:

```sh
git diff rust-v0.172.0 HEAD
git log --first-parent --oneline rust-v0.169.0..HEAD
```

## Runtime and distribution

- The app-server binary is named `mekugi-app-server`. The shipped runtime also
  includes `codex-code-mode-host` and, on Linux, `bwrap`. Upstream clients and
  SDKs remain in the source tree but are not shipped or maintained as part of
  Mekugi. The [Cargo workspace](codex-rs/Cargo.toml) and lockfile cover the
  native build; the fork uses Mekugi's release process, not upstream release CI.
- `codex-arg0` dispatches the native Rust `mcat`, `mchanges`, `mfinspect`,
  `msymbol`, and `mrun` CLIs (`codex-mekugi-tools`) through stock PATH symlinks
  and exports
  `CODEX_ARG0_PATH_DIR` so login shells retain those aliases on `PATH`. It also
  stages per-session `vcs-guard/` (`git`, `gh`, `hg`, `svn`, `jj`) and
  `sudo-guard/` (`sudo`, `gpg`, `gpg2`, `mekugi-sudo-askpass`) symlinks
  dispatched by `codex-shell-guard` without adding them to the process-wide
  `PATH`.
- Native shell guarding (`codex-shell-guard`, gated by `features.shell_guard`,
  default enabled in interactive sessions and disabled for headless/internal
  runs) prepends the staged guard directories in `codex-core` after user
  `PreToolUse` hooks run and applies a minimal `tree-sitter-bash` executable-word
  rewriter for explicit paths, `env -i`, `command -p`, `PATH` overrides, and
  nested `-c` shells without a patched shell or `BASH_ENV` function injection.
  The session tracking hook instruments the command selected by user hooks;
  competing user rewrites still resolve in native completion order.
  Shims communicate over a session-private Unix socket (`codex-uds`) bridged by
  `mekugi-app-server` to App-Server Protocol v2 `shellGuard/requestApproval` and
  `shellGuard/requestSecret` requests, keeping secrets zeroized and excluded
  from rollouts, tracing, and analytics.
- Native coordination uses the shared thread manager for live foreign messages,
  side questions, listing and inspection. Child startup, wait-progress selection
  and final change delivery no longer request resources from a connected Go
  client. Preload and context reset resolve retained change IDs from the
  native store without a launcher opt-in. The retired resource
  callbacks are no longer on the v2 wire.
- Native change-evidence capture (`codex-change-history`, `MEKUGI-806`):
  `codex-core` captures pre-edit baseline bytes and post-edit observed bytes at
  the `apply_patch` tool boundary (covering direct calls, intercepted shell
  patches, partial writes before failing exits, and JavaScript code-mode `exec`
  cells), persists immutable content-addressed blobs and monotonic stream-scoped
  change records (`amber1`, ...) before publishing tool receipts (surfacing
  storage failures explicitly), exports `MEKUGI_CHANGE_HISTORY_DIR` to child
  shells, and backs the native `mchanges` CLI (`--list`, composed diff review,
  `--summary`, `--history`, and path-scoped `revert`/`apply` preserving
  unrelated edits and failing explicitly on drift or corrupt/missing evidence
  without conflict markers).
- Native exec tracking and shell-write / `mktemp` observation (`codex-exec-track`,
  `MEKUGI-801`): `codex-core` selects authored command spans with
  `codex-shell-guard::parse_bash` for supported Bash and Linux `dash`-backed
  `sh` lists, instruments segment boundaries inside one real shell without
  fixed inherited caller FDs (3/4/7/8) or held tracking FDs across detached
  descendants, preserves TTYs intact (status/timing only), falls back cleanly
  to stock combined output on unsupported syntax or lost observation, captures
  actual successful `mktemp` files and directory trees independently of
  printable output (ignoring dry runs and failures), and persists observed
  shell writes (including writes before nonzero exits) to `codex-change-history`
  under `ChangeSourceKind::ShellWrite` with shared `claim_key` deduplication.
- Subprocess regression tests are in a separate
  [crate](codex-rs/app-server-subprocess-tests/Cargo.toml) and exercise the staged
  runtime, including the exec-server fixture's Linux sandbox helper so it can
  find the bundled `bwrap` without a system installation. The native development
  profile favors incremental rebuilds.

## Execution and model guidance

- Both question tools share rich options (preview and explicit recommendation) and
  optional headings/free-text-only questions. Runtime-assigned qN IDs continue
  across context resets, forks and resume. Answers are compact indexed objects;
  async replies use `<question_answers>`, without expansion after context reset.
  The v2 response now carries an ordered answer vector, not a string-answer map;
  clients must migrate. Catalog parameter overrides cannot replace this schema.
  Persisted pre-upgrade async questions still decode title/string-option records;
  serialization uses only the new shape and retained authored IDs do not reset
  the runtime question counter.

- Tool, unified-exec, and code-mode output uses stock upstream middle
  truncation and spills the omitted portion to
  `$SESSION_TMP/truncated_output/{id}` (created on demand), adding only a short
  `(id: {id})` marker inline and stating the directory once in session shell
  helper guidance.

- Native [feature policy](codex-rs/features/src/lib.rs) fixes JavaScript code
  mode as the only model tool interface, with native V2 agents, token budgeting,
  step model switching, and reasoning-effort overrides. Stock direct tool mode
  is unsupported. Goals, Apps, agent message boards, Guardian V3, and the stock
  sleep tool are disabled; Codex feedback operations are unsupported.
- The fork owns default model instructions even when capabilities come from a
  remote catalog. Personality comes from user instructions. Native guidance
  describes available Mekugi shell helpers and can guide substantial child
  replies through `child_reply_summary`. Plugin discovery is explicit rather
  than advertised automatically in model context. The managed host catalog uses
  concise base skill guidance. Code-mode batching lives in the exec description.
- Code-mode waits can hold the observer while a native wait pauses the cell
  deadline. The paired host and runtime exchange observation liveness separately
  from stream heartbeats. Instant-interrupt steering defers stream preemption
  until an in-progress assistant message completes and suppresses streaming of
  partial items while draining a steered response, preventing mid-truncated
  assistant messages. Provider retry changes surface the first capacity wait,
  can resume a usage-limited turn after reset or retry advice, format multi-hour
  and multi-day waits in hours/days, and fall back to default `x-codex-*` window
  headers when `x-codex-active-limit` has no dedicated window prefix.

## Context and retained sessions

- Forks retain inherited context occupancy but start independent provider
  spending totals, including when the source was compacted. Native response
  records retain per-thread usage, output timing and reference OpenAI costs.
  A response completed without usage leaves a coverage gap. Spending
  updates leave context occupancy unchanged.
- Mekugi sends requests directly to the selected provider. A custom route
  requires an endpoint, cannot redirect inference or catalog requests, and has
  no built-in price. Native exchange measurements feed launch-wide diagnostics
  without retaining prompts, tool input, or response text in the private log.
- Native code-mode input events feed provisional live diffs. Legacy saved Diff
  does not include newly native-captured edits; use native `mchanges` and
  Activity patch details to review them.
- When session naming is enabled, new persistent Main threads can receive a
  native title from a one-shot, untooled request on their selected provider.
  An existing manual name wins; title failure does not fail the turn.
- Native request construction expands submitted attachment snapshots and
  attributes standalone tool context without rewriting retained review items.
  Side forks reuse parent cache routing; answer-only forks stop when auto
  reset is required. UI BTW caps supported effort at low. Native exact-model
  tier defaults and thread choices use provider capability checks. Resuming
  a stopped session keeps explicit choices and uses current defaults otherwise.

- Context resets rebuild from current sources without a generated summary.
  `new_context` can reload selected files, skills, and captured changes. Manual
  resets are cancellable and report settlement; reset receipts retain which
  sources were acquired. Accepted user input and agent handoffs can also attach
  sources through the same native acquisition path.
- The runtime captures structured environment facts, prepares the shared
  `SESSION_TMP`, and appends it to `PATH` (reported via `on_path="true"` on
  `<session_tmp>`). The native skills extension uses the local `skills-mgr`
  catalog where configured and otherwise retains stock skill discovery.
- Native shell capture uses the runtime's `SESSION_TMP` path to resolve
  direct output redirects without changing ordinary hook input.
- Model requests reuse repeated tool output and source text without replacing
  the original retained records, preferring a single-base unified diff over
  fragmented multi-segment references when savings stay within a bounded
  clarity budget. Detected `AGENTS.md` instruction updates are delivered as a
  compact unified diff against the previously delivered snapshot when doing so
  saves at least 128 bytes compared to repeating the full instructions (and emit
  a concise unchanged notice on directory-only changes), falling back to full
  replacement for complete rewrites or legacy sessions without a baseline.
  Context usage notices and deduplication savings come from native observations
  instead of host prompt rewriting. Mekugi only promises resume for Mekugi
  sessions.
- Migration 63 records completed compactions in Mekugi state. The upstream
  0.171.0 state migrations were moved to 64 and 65 to preserve the recorded
  checksum in existing Mekugi databases.

## Native collaboration and protocol

- Native thread start and cold resume accept a private caller environment and
  named profile. Concurrent families keep separate settings and credentials;
  live reattach verifies the selector without reopening its profile.
  Thread-scoped configuration reads expose only terminal settings and preset
  ownership metadata.
- V2 delegation supports recipient-authorized preload, agent and subtree
  inspection, and foreign messages routed through their original resource
  owner. Side questions and progress summaries use a tool-free auxiliary
  session. Native waits support any/all, yield progress, and are separate from
  context resets.
- Native delivery keeps accepted outbound mail separate from model history and
  preserves inbound plaintext mail for live and resumed transcripts. Progress
  and final replies can attach retained change ranges; delivery receipts
  distinguish what was rendered and sampled in the recipient's context.
- The [JSON-RPC protocol](codex-rs/app-server-protocol/src/protocol/common.rs)
  adds thread inspection and subtree listing, agent end,
  context-reset cancellation and events, mid-turn `beforeItemId` revert targets
  on `thread/revert`, and feature-use reads. Native observation supplies assignment,
  usage, and progress facts to Mekugi without a second agent registry.
  A subtree read during child startup returns its readable members without
  waiting for the child or reporting the parent missing.

## Upstream imports

| Release | Fork commit |
| --- | --- |
| `rust-v0.172.0` | Pending merge (`chore/upstream-rust-v0.172.0`) |
| `rust-v0.171.0` | `221fbc5eed` |
| `rust-v0.170.0` | `a9129794eb` |
| `rust-v0.169.1` | `518e161a8d` |
| `rust-v0.169.0` | Initial upstream baseline |
