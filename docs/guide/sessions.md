# Sessions and recovery

Resume, branch and rewind conversations, and understand compaction and retained storage.

An empty transcript is a draft, not a session. A session starts once you send
the first message and Codex saves it. Mekugi does not assign a session ID to a
draft; any native thread ID that stock Codex already assigned stays unchanged.

## Resume

`resume THREAD_ID` attaches to a saved thread if it is still running on the
local server, or restores its session otherwise. `resume --last` selects the latest
saved session; bare `resume` opens the
workspace picker. Type to search, Tab for all workspaces, Enter to resume. Inside an
idle session, `/resume` opens the picker or `/resume THREAD_ID` switches directly. Saved
model/effort/tier, layout, roster, child history, and retained Diff return; explicit
`-m`/`-c` overrides named settings. Drafts, selections, filters, and scroll positions do
not return.

`/fork` asks Codex to branch the conversation and switches to the new thread
if Codex accepts it. With `/fork PROMPT`, the trailing text is sent to the
new thread. The original stays resumable. A failed fork keeps the original
session usable.

`/rewind` opens a restore-point dialog in the terminal and browser while idle,
after the first user turn. It lists turn starts and mid-turn steered messages.
Select a point to remove it and all later conversation from the same session,
keeping current settings and pending input. It requires paginated Codex
history; legacy sessions are refused before edits change.

Choose Keep edits (the default) or Revert edits to undo eligible captures from
that point onward, including descendant agents. Without such captures only
Keep edits is available.
Revert preserves unrelated edits when they can be merged. Missing evidence
disables Revert; conflicts stop the rewind. Keep remains available. If the
history replacement fails after file undo, the server attempts to restore the
pre-undo files and reports any conflict that prevents restoration. Retained
change and output evidence stays available, including any recorded reversal.

Rewind keeps the session identity and descendants. Codex reloads the caller's
runtime, so its process handles may stop working. No saved branch or archive
confirmation is created.

## Compaction and recovery

In the terminal or browser, submit `/compact` with Enter to compact the selected
thread now. If work is active, Enter interrupts it first. With the command in
the composer, press Tab during active work to queue it without interrupting.
After a successful busy-thread compact, waiting drafts take precedence.
If none are waiting, the thread continues without adding a user message.

Manual `/compact`, context-full compaction and agent checkpoints reset Codex
context without generating a handover summary. Agents may select files, line ranges,
skills and captured-change IDs through native `functions.new_context`, or recover
needed facts through ordinary reads after reset. Requested resets are not vetoed
by a cost estimate. `wait_agent` has no reset or reload fields. Use standalone
`functions.new_context` for checkpoints; same-cell reset-and-wait ordering is not
established, so a reset acknowledgement is not proof that reset has finished.

The latest reload selection also applies to later manual and automatic resets.
Omitting `reload` keeps it; `reload:{sources:[]}` clears it. Temporary task-state
JSON is optional and read only when selected, with selected ranges and nested
references determining what is acquired.

Selected reload and fresh-delegate preload text can include `@path` references to
required files, including inside JSON state. Bare paths do not trigger reads. The
selected source counts as depth 1; depth 3 is the last acquired level. References
resolve beside native source files, or from the workspace for managed skills.
Skills can select a reference such as `name/references/guide.md` instead of the
whole skill. Captured diffs and unselected lines do not expand references.

Before the next model request, native reset acquires selected sources with the thread's
permissions. Failed reads stay visible without blocking the others. Codex supplies
`AGENTS.md`, so it is excluded from reload. Captured reloads survive resume and fork
without rereading merely because the thread resumes; the next reset reads current
sources. Earlier saved sessions retain their compaction summaries, available
from **Context compacted** in the transcript. See the
[compaction contract](../reference/agent-guidance.md#compaction-context) (note that stock
Codex startup still reads a caller-configured `experimental_compact_prompt_file` even though native
reset ignores it).
