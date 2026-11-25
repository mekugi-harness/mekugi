# Module reference

Generated from feature descriptors, Codex binding declarations and application
composition. 12 of 12 feature modules are selected.
Regenerate with `just generate`; verify with `just check-generated`.

The [composition contract](../contracts/composition.md) defines ownership and
lifecycle. Declared connections and requirements do not establish installed-host
availability. Each module's specification owns its behavior and acceptance.

## [activity](../../mekugi-tui/modules/activity/SPEC.md)

Own host-operation feeds, roster derivation and retained output

- Composition: selected; lifetime: `attached-session`.
- Requires: none.
- Contributes: `record-kinds`, `terminal-roster`.
- Records: `wait`.
- Codex connections: none.

## [agentmetrics](../../mekugi-tui/modules/agentmetrics/SPEC.md)

Measure reader invocations, observed edits and recorded host completion

- Composition: selected; lifetime: `stateless`.
- Requires: none.
- Contributes: `inspection`, `diagnostic-sections`.
- Codex connections: none.

## [changehistory](../../mekugi-tui/modules/changehistory/SPEC.md)

Capture, retain, review, apply, and revert filesystem change evidence

- Composition: selected; lifetime: `invocation`.
- Requires: `sessionhistory`.
- Resources: private workspace checkpoints and bounded warm-up tasks.
- Contributes: `commands`, `record-kinds`, `terminal-auxiliary`.
- Commands: `mchanges`.
- Records: `changes`, `change-stage`, `change-evidence`, `change-file`, `change-text`.
- Codex connections: `executable-frontend`.

## [commandsegments](../../mekugi-tui/modules/commandsegments/SPEC.md)

Receive, retain, and restore command-segment observations

- Composition: selected; lifetime: `invocation`.
- Requires: `sessionhistory`.
- Resources: optional private report FIFOs and consumer tasks.
- Contributes: `record-kinds`.
- Records: `commandsegments`.
- Codex connections: none.

## [conversation](../../mekugi-tui/modules/conversation/SPEC.md)

Own conversation drafts, transcripts, prompts, queues, and side answers

- Composition: selected; lifetime: `attached-session`.
- Requires: none.
- Codex connections: `observation`.

## [diagnostics](../../mekugi-tui/modules/diagnostics/SPEC.md)

Retain and inspect private diagnostic evidence

- Composition: selected; lifetime: `invocation`.
- Requires: `sessionhistory`.
- Hosts: `diagnostic-sections`.
- Codex connections: none.

## [livepreview](../../mekugi-tui/modules/livepreview/SPEC.md)

Live preview view types and terminal renderer

- Composition: selected; lifetime: `stateless`.
- Requires: none.
- Codex connections: none.

## [notifications](../../mekugi-tui/modules/notifications/SPEC.md)

Deliver bounded notices, failure details and recovery summaries

- Composition: selected; lifetime: `invocation`.
- Requires: none.
- Codex connections: none.

## [sessionhistory](../../mekugi-tui/modules/sessionhistory/SPEC.md)

Own retained session publication, namespaces, allocation, leases, and retention

- Composition: selected; lifetime: `invocation`.
- Requires: none.
- Contributes: `record-kinds`.
- Hosts: `record-kinds`.
- Records: `hooks`, `compaction`, `compact`, `frozen-file`, `frozen-resource`, `output`, `cursor`, `call`, `commentary`.
- Codex connections: none.

## [sessionpreferences](../../mekugi-tui/modules/sessionpreferences/SPEC.md)

Retain applied settings and renderer preferences for a workspace and thread

- Composition: selected; lifetime: `invocation`.
- Requires: `sessionhistory`.
- Codex connections: none.

## [transportdiagnostics](../../mekugi-tui/modules/transportdiagnostics/SPEC.md)

Summarize native provider responses and read private diagnostic evidence

- Composition: selected; lifetime: `invocation`.
- Requires: none.
- Contributes: `record-kinds`, `inspection`, `diagnostic-sections`.
- Records: `request-timing`.
- Codex connections: none.

## [usage](../../mekugi-tui/modules/usage/SPEC.md)

Project provider usage and reference costs from native responses

- Composition: selected; lifetime: `invocation`.
- Requires: none.
- Resources: on-demand retained-usage publication worker.
- Contributes: `record-kinds`, `presentation`.
- Records: `usage`.
- Codex connections: none.

