# Native evidence and validation

Status: Accepted

## Context

The native runtime owns session and execution facts that both clients need to
present. Restoring useful evidence from the earlier clients must not recreate a
separate Go runtime or turn every historical presentation into a requirement.
The clients have different Details and navigation surfaces, but their evidence
must retain its native identity and scope. See
[ADR 0001](0001-client-interaction-parity.md) for the controls and navigation.

## Decision

### Evidence and presentation

Use native and stock capabilities where they supply the behavior. Native owns
command, change, usage, process and coordination facts; clients present those
facts without inventing measurements or changing their scope.

Both clients show captured-edit provenance and signed change counts. Web Live
shows available edits during patch, shell and delegated work, labelled by
caller, and hands settled changes to paged Saved Diff, filtered by caller. Web
also shows each agent's edits and skills. Navigate directly to a child thread,
without a combined or fallback agent transcript. Terminal Details retains
saved command and edit receipts, including source and caller. Run links open
the requested output range; file links and errors open the selected range or
error.

Command presentation preserves effective working directories and distinct
command segments. Recognized steps retain their activity labels. Both clients
show a multi-step invocation's output and exit status once, in Details; the
transcript does not claim that individual steps ran or exited separately.
An activity row finishing does not establish command completion: an ongoing
command remains Running.
Preserve the actual `-1` command result for approval denial and timeout.

Web file links open Details at the linked line or range within the selected
thread's workspace. Run links resolve the original retained output by run
identity and range; unavailable ranges are identified as such. Read permitted
files with bounded, nonblocking reads and check the opened file. Older history
stays quiet and paged, with the scroll position preserved.

Render native tool results according to their type. Web Details retains the
full MCP result envelope, dynamic-tool rows render supported native media, and
user and generated images have inline markers and previews. Collaboration
traffic has distinct event rows with participants and associated evidence.
Show public reasoning summaries, not raw reasoning. The native plan remains
prose, without a separate checklist. Distinguish compaction from context
reset and disclose retained summaries, sources, context and time when known.

Terminal process views cover the thread family, update live, including commands
that outlive their turn, and retain settled rows until closed. Reuse stock
background-terminal list and terminate RPCs. Stop addresses the selected
invocation, so an expired selection must not stop a different command that
reused its process ID.

### Rewind and evidence integrity

Both clients offer a stored user-message chooser. Keeping edits is always
available; reverting edits requires usable captured evidence. The native
`thread/revert` operation owns workspace mutation and rollback, accepts the
optional expected change IDs, and revalidates the capture. If history
replacement fails after file undo, it compensates the file changes. History
selection may be mid-turn.

### Measurements

Restore saved-session usage and distinguish complete from partial evidence.
Show actual measurement times when known, not local receipt times. Show input,
cached input, uncached input, output and reasoning tokens separately, alongside
context, cost, native turns and roundtrips. Launch request and timing totals do
not change when switching to One-off or a delegate. Feature counters compare
the selected thread's own actions with its descendants.
Present aggregate metrics without per-exchange drilldown or a Transport tab.
Keep the debug path, not historical debug write counters. Show available
rate-limit resets, credits and additional native limits. Retain the roster's
activity line and named measurements without adding a generic status row.

### Validation

Port material earlier snapshot cases and check every saved snapshot file has
an exercising test. Restore material welcome and first-screen release checks.
The native preview uses a real terminal and native runtime; its scripted
provider emits `request_user_input_async` and exercises its picker. Host
acceptance retains the material earlier `test-host` outcomes alongside current
checks. CI acceptance uses a real browser and native app-server, including the
Web-to-terminal shell-approval crossing, not only compilation.

## Consequences

Native evidence and identity must survive live delivery, retention and resume,
including caller attribution and the Live-to-Saved transition. A UI completion
or receipt cannot stand in for a command result, a measurement time or a
process identity. Rewind must preserve workspace and history consistency on
failure. Validation covers the actual cross-client boundary as well as
projection and snapshot behavior; a compile or isolated rendering check alone
does not establish that boundary.
