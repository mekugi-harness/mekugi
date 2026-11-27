# Client interaction parity

Status: Accepted

## Context

The native migration changed interactions in both clients. Parity means
preserving the chosen user workflows, not reproducing every historical
implementation or exposing the same commands in both interfaces. The decisions
below apply to [issue #59](https://github.com/mekugi-harness/mekugi/issues/59)
and [PR #60](https://github.com/mekugi-harness/mekugi/pull/60).

A session starts when its first message is sent and saved. An empty transcript
is not a session, although a stock Codex thread identity allocated before that
point remains unchanged. Browser discovery follows stock behavior: it need not
discover or join an unsaved terminal thread. The browser lists active and saved
non-archived threads by default and opens a listed thread directly, without a
Resume command, picker, or manual ID entry.

## Decision

### Browser controls and commands

The supported browser slash commands are `/btw`, `/oneoff`, `/ps`, `/rewind`,
`/archive`, and `/compact`. Slash discovery starts by typing; there is no slash
toolbar button. Exact-name Enter executes a command, and an unknown slash
command produces local feedback rather than being sent to the model.

Use the composer settings controls for independent Model and Effort controls,
Permissions, and Fast. Choosing a model does not open Effort automatically.
Show Fast only when the selected model advertises the corresponding tier. Put
Predictions in the Display popover; it controls native prediction behavior, not
just a saved preference. Use Rename and Fork in Session details, with visible
fork failure feedback; retain the existing details popover for status and
session metrics. Presets have a button and full slot management, and named
presets also appear in `$` completion. Skills use `$` completion, not a browser
management command. New Thread and ordinary browser navigation or close replace
browser Clear and Quit commands.

There are no browser slash commands for these controls, for `/live`, or for
the hidden `/copy`, `/lock`, and `/unlock` operations. Neither client gains
`/fast`, `/rename`, `/new`, `/exit`, `/one-off`, or `/side` aliases.

### Terminal controls and commands

Model and Effort target the selected Main or One-off, including effort keys;
Tier targets Main only. Offer only advertised effort choices and tiers. Label
a cleared tier Default; do not invent a Fast fallback. Accept settings updates
during work where stock Codex does. Use stock configuration updates where
supported; otherwise settings apply to later turns. A provider request already
sent is unchanged.

`/model` is a bare picker, with no argument or flag, including when a preset
supplies input. Presets retain their admission guards and supported model,
effort, and tier seeds. Permissions retain the existing picker and optional
inline profile, policy, and reviewer syntax.

`/skills` opens the stock List skills and Enable/Disable Skills choices; List
opens the `$` completion picker. `/copy` offers the whole completed response or
its copyable parts, scoped to the displayed source. `/live`
and `/predictions` are bare toggles; `/lock` and `/unlock` are exact bare
commands. Slash discovery is fuzzy and inserts a trailing space on selection;
Tab selects a completion without executing it.
Terminal `/status` includes provider, permissions, and instruction sources;
`/session` remains the session metrics entry.

Terminal `/fork [<prompt>]` lets the native operation decide admission, and
trailing text is the prompt for the fork. `/resume` follows stock admission and
listing behavior, with workspace, age, and branch metadata. Replay and
inspect-session are not restored. `/clear` requires a ready, idle thread and
starts a fresh thread with the current workspace, provider, model, effort, and
tier, using the launch approval mode. `/quit` prompts when Main, any delegate,
or any command is running; `/detach` remains separate.

### Composer, queue, and approvals

Both clients retain the workspace file catalog. Terminal `@` binds files and
images with path quoting and atomic undo. Terminal `$` skill completion binds
the exact skill path and restores the unfinished token in one Undo. Browser
skill completion shows friendly labels and rebinds before sending.

Every browser attachment has both a chip and an inline placeholder. A selected
message used as a Reference is not an attachment: it has an inline placeholder
without a chip. Browser uploads support bounded UTF-8 text as well as images;
image recognition uses file bytes rather than a filename extension. User and
generated images have inline markers and previews. Browser questions retain
separate image drafts.

Browser Enter sends or steers; busy Tab queues. Shell mode has a button, label,
highlighting, and no attachments. Tab queues a shell draft even when idle;
Enter executes it immediately. Browser waiting input supports Edit and Steer,
and rejected drafts support Retry, Edit, and Discard with attachments.
The terminal shows the full waiting queue and permits editing. Queued shell
commands use the durable FIFO, with refused input restored. Steer a queued
message through the stock turn operation and delete it only after success.
Do not pause the queue just because the permissions picker is open. Preserve
Web drafts across settings changes, require native confirmation, and show
reconnect or failure feedback.

### Continuity and side workflows

Both clients provide `/btw` as a separate answer dock with follow-ups and Close.
One-off requires a task; an existing One-off offers Cancel or Replace, with
Replace starting a fresh snapshot. Browser Main and One-off have separate tabs
and drafts, and the One-off can be closed. Browser `/ps` points to the process
dock with individual Stop; terminal processes cover the family, update live,
retain settled rows, and stop the selected process.

Both clients' Rewind chooser includes stored user messages, including mid-turn
messages. Keep edits is available; Revert edits is offered only when usable
captured changes exist. [ADR 0002](0002-native-evidence-and-validation.md)
records how the server keeps Rewind consistent. Archive confirms and warns
about active work; the browser has no separate End work action. Compact can
interrupt and continue active work, or be queued with Tab until idle.

Browser Reference works for transcript and Details selections. Terminal
Reference retains its current selection effect. Terminal saved-draft recall
uses native input history without legacy import or the old exit combination.
Dismiss only the displayed terminal notice revision. The browser workspace
selector offers machine-derived choices and manual entry.

## Consequences and non-goals

The clients share native capabilities without requiring identical command
surfaces. A browser user finds settings, presets, status, and thread opening in
controls rather than slash aliases; terminal users retain the specified command
and picker workflows. Stock discovery does not promise that an empty terminal
transcript appears in the browser. Rewind does not promise file undo without
usable capture, and settings changes cannot alter a provider request already
sent.

This decision does not restore replay, inspect-session, legacy draft imports,
or cross-product session resume.
