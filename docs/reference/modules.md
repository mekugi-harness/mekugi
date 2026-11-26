# Module reference

Generated from workspace module membership and module specifications.
6 feature modules are present in the Go workspace.
Regenerate with `just generate`; verify with `just check-generated`.

The [composition contract](../contracts/composition.md) defines ownership and
lifecycle. Each module's specification owns its behavior and acceptance.

## [activity](../../mekugi-tui/modules/activity/SPEC.md)

- Present host operations, tool invocations, agent traffic, and captured edits clearly without altering stock tool inputs, results, batch/parallel execution, or model context. - Identify supported tools in the roster while their input is still streaming. - Show accurate sender/recipient direction for agent traffic; hide internal Mekugi bookkeeping without losing inseparable ordinary command output; never duplicate an earlier reply on interruption.

- Packages: `feed`, `presentation`, `terminalview`.

## [changehistory](../../mekugi-tui/modules/changehistory/SPEC.md)

- Passively capture observed edits from stock `apply_patch` and shell writes under durable short change IDs, supporting cross-agent review across each record's authored workspace. - Support surgical reversal (`revert`) and reapplication (`apply`) of selected captured edits (including path-scoped and `/rewind` reversals) without discarding unrelated or uncommitted work. - Keep model-authored rewind scoped to the caller's own captured edits.

- Packages: `presentation`, `terminalview`.

## [conversation](../../mekugi-tui/modules/conversation/SPEC.md)

- Provide unified composer editing across Main, live delegates, One-off, questions, and approval denials with atomic file, skill, selection, and image attachments (including image previews for host paths, uploads, and paste) and undo/redo/external-editor support. - Queued composer input must not cancel an active logical agent wait. - Support optional `mcat` recall of prior selections, skills, files, and submitted-image references via retained read-only files after context resets/compactions without replaying image pixels or re-reading unselected file bodies; deduplicate identical supplied source content. - Support isolated `/oneoff` work-capable forks and read-only `/btw` side questions without polluting Main's transcript.

- Packages: `codexbinding`, `completion`, `composer`, `presentation`, `presets`, `prompt`, `terminalview`.

## [livepreview](../../mekugi-tui/modules/livepreview/SPEC.md)

- Stream provisional live diffs while the agent writes stock patches and supported shell edits, before host completion. - Allow preview code content to scroll independently while keeping file headers accessible. - Previews are strictly display-only: they never execute source, allocate change IDs, or establish durable evidence.

- Packages: `preview`, `terminalview`.

## [notifications](../../mekugi-tui/modules/notifications/SPEC.md)

Inherited from product requirements: surface actionable failure and progress notices (and generic desktop wake-up alerts) reliably without dropping unseen errors, duplicating alerts, leaking private content into desktop notifications, or affecting execution outcomes.

- Packages: `presentation`.

## [usage](../../mekugi-tui/modules/usage/SPEC.md)

- Track cumulative per-thread provider-authoritative token usage, reference API cost estimates, and measured output throughput alongside execution without adding usage text to model context or writing Markdown usage files. - Keep missing or partial usage/pricing explicitly labeled (never inferred as zero or confused with provider billing quotes).

- Packages: `presentation`.

