# Change evidence

## Requirements

- Retain live diffs and durable captured-change review for stock `apply_patch` and shell writes
  without replacing native execution, batching, tool arguments, or host results.
- Preserve durable evidence and retention across optimizations; missing or incomplete evidence must
  be labeled explicitly, never presented as complete or as a no-op.

## Boundary contract

- **Observation only**: Codex owns editing, execution, permissions, and continuations. Capture never
  replays effects, alters tool arguments/results, or completes a yielded call before its terminal
  continuation. Failed commands keep any bytes they actually wrote.
- **Publication before reference**: immutable per-file evidence is persisted before change IDs or
  completed receipts become usable. Storage failure never advertises durable evidence; missing or
  corrupt records fail explicitly rather than becoming empty diffs.
- **Coverage honesty**: captured evidence describes observed file differences within witnessed
  windows, not a live Git diff or proof that unobserved effects did not occur. Concurrent external
  writers cannot be separated without host per-write facts; missing baselines, expired reads, or
  unavailable checkpoints retain explicit incomplete/partial coverage.
- **Provenance separation**: VCS operations that import, discard, or write repository state are
  diagnostic history (`mchanges --history`), excluded from authored edit counts, saved Diff, rewind,
  and child handoffs. Named interpreter writes in mixed windows remain authored.
- **Delivery**: change IDs are appended only after persistence without modifying original host text;
  output deduplication runs only after observation/retention consume originals. Provisional live
  previews never establish durable evidence.
- **Activity attribution**: code-mode shell receipts join exact native command
  items, independently of command exit status. If those identities are
  unavailable, the diff remains saved and capture history reports the missing
  attribution.

## Current approach (replaceable)

- Native capture reads patch baselines and shell write targets at their tool
  boundaries, including dynamic patches inside `exec` cells. Shell targets
  include tracked `mktemp` files and direct redirects through `SESSION_TMP`.
  Evidence is written to the native store before an ID is exposed.
- Live Diff streams authored edits as the model generates tool call inputs
  (`apply_patch` and streaming shell/Python text edits such as `cat`/`tee`
  heredocs and Python `open`/`read_text` -> `replace` -> `write_text`/`write`)
  through `item/fileChange/patchUpdated`. Non-streaming commands whose bytes
  come from disk (`cp`, `cat < file > out`, `shutil.copy*`, user shell,
  formatters, etc.) do not open Live Diff; their observed writes remain
  available in Saved Diff via `exec-track`.
- Native `mchanges`, preload, context reset and agent handoffs consume that
  store. Both clients read saved changes, including user shell edits, through
  `thread/changes/read`. A capture can be attributed to an agent or to an
  unattributed execution such as user shell.
- The root's saved review covers its family. A child reads its own captured
  changes. Caller pages keep an independent edit separate from writes by
  other callers. Command Details join evidence by thread, turn, and item,
  not by file name or exit status.
- See [Change history](../../mekugi-tui/modules/changehistory/SPEC.md) for
  `mchanges` and saved review.
