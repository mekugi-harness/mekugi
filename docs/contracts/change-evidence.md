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

## Current approach (replaceable)

- Native capture reads patch baselines and shell write targets at their tool
  boundaries, including dynamic patches inside `exec` cells. Shell targets
  include tracked `mktemp` files and direct redirects through `SESSION_TMP`.
  Evidence is written to the native store before an ID is exposed.
- Native `mchanges`, preload, context reset and agent handoffs consume that
  store. Legacy saved Diff still opens retained client records but does not
  include newly native-captured edits. Activity shows native patch details.
- The client still captures explicit user shell actions for saved review.
  It does not reconstruct model tool execution from inference traffic.
- See [Change history](../../mekugi-tui/modules/changehistory/SPEC.md) for
  `mchanges` and saved review.
