<script lang="ts">
  import { untrack } from "svelte";
  import type { InitialInput } from "./attachments";
  import Composer from "./Composer.svelte";
  import DetailsPanel from "./DetailsPanel.svelte";
  import DiffPanel from "./DiffPanel.svelte";
  import DisplayPopover from "./DisplayPopover.svelte";
  import { basename, statusText } from "./format";
  import type { Machine } from "./hub.svelte";
  import { editEntryPaths, isEditEntry } from "./kinds";
  import Path from "./Path.svelte";
  import { preferences } from "./preferences.svelte";
  import Prompt from "./Prompt.svelte";
  import type { Entry } from "./protocol";
  import Roster from "./Roster.svelte";
  import { semantic } from "./semantic.svelte";
  import SessionInfo from "./SessionInfo.svelte";
  import { stream } from "./stream.svelte";
  import ThreadDocks from "./ThreadDocks.svelte";
  import Transcript from "./Transcript.svelte";
  import Alert from "./ui/Alert.svelte";
  import { Badge } from "./ui/badge";
  import Button from "./ui/Button.svelte";
  import Dialog from "./ui/Dialog.svelte";
  import Dot from "./ui/Dot.svelte";
  import Field from "./ui/Field.svelte";
  import Popover from "./ui/Popover.svelte";
  import { Separator } from "./ui/separator";
  import { input as inputStyle } from "./ui/styles";
  import { show, view } from "./view.svelte";

  let {
    machine,
    id,
    initial,
    oninitial,
    sidebar,
    onsidebar,
    onend,
  }: {
    machine: Machine;
    id: string;
    initial?: InitialInput;
    oninitial: () => void;
    sidebar: boolean;
    onsidebar: () => void;
    onend: (target: { machine: Machine; id: string; title: string }) => void;
  } = $props();

  let selectedQuestion = $state("");
  let panel = $state<"agents" | "changes" | "details" | undefined>();
  let selectedEntry = $state<Entry | undefined>();
  let focusChangePath = $state<string>();
  let hiddenHooks = $state(0);
  let renameOpen = $state(false);
  let renameValue = $state("");
  let savingName = $state(false);
  let renameError = $state<string | undefined>();
  let wall = $state(Date.now());
  let narrow = $state(false);

  $effect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(max-width: 68.74rem)");
    const sync = () => (narrow = mq.matches);
    sync();
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  });

  $effect(() => {
    const timer = setInterval(() => (wall = Date.now()), 1000);
    return () => clearInterval(timer);
  });

  $effect(() => {
    const thread = id;
    const owner = machine;
    untrack(() => {
      stream.connect(thread, owner, stream.family?.threads.includes(thread));
      semantic.connect(thread, owner);
      selectedEntry = undefined;
      renameOpen = false;
      renameError = undefined;
      savingName = false;
    });
    return () => {
      stream.close();
      semantic.release(thread, owner);
    };
  });

  const session = $derived(stream.session);
  const title = $derived(
    session?.title || (session?.workspace ? basename(session.workspace) : undefined) || id,
  );
  const root = $derived(
    stream.roster.agents.find((a) => a.name === "/root") ?? stream.roster.agents[0],
  );
  const mainEntries = $derived(
    stream.main.order
      .map((seq) => stream.main.entries[seq])
      .filter((entry): entry is Entry => !!entry),
  );
  const status = $derived(statusText(stream.status));
  const questionChannel = $derived(
    (semantic.prompts?.questions ?? []).flatMap((call) =>
      call.questions.map((question, index) => ({ call, question, index })),
    ),
  );
  const questionPosition = $derived(
    Math.max(
      0,
      questionChannel.findIndex((item) => item.question.id === selectedQuestion),
    ),
  );
  const currentQuestion = $derived(questionChannel[questionPosition]);
  const hasPrompts = $derived(
    !!(semantic.prompts.approvals.length || semantic.prompts.questions.length),
  );
  const net = $derived(stream.review?.counts.Net);
  const hasChanges = $derived(!!stream.review?.files.length);

  function openChanges(paths: string[] = []) {
    focusChangePath = paths[0];
    panel = "changes";
  }

  function openDetails(entry: Entry) {
    if (isEditEntry(entry) && hasChanges) {
      openChanges(editEntryPaths(entry));
      return;
    }
    selectedEntry = entry;
    panel = "details";
  }

  function onkeydown(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.shiftKey && !event.altKey) {
      const key = event.key.toLowerCase();
      if (key === "h") {
        event.preventDefault();
        preferences.toggleHooks();
        return;
      }
      if (key === "d") {
        event.preventDefault();
        preferences.toggleDebug();
        return;
      }
    }
    if ((event.metaKey || event.ctrlKey) && !event.altKey && !event.shiftKey && event.key === ".") {
      event.preventDefault();
      panel = panel ? undefined : "agents";
    }
  }

  async function saveRename() {
    const trimmed = renameValue.trim();
    if (!trimmed || savingName) return;
    savingName = true;
    renameError = undefined;
    const err = await semantic.rename(trimmed);
    savingName = false;
    if (err) {
      renameError = err;
      return;
    }
    renameOpen = false;
  }
</script>

<svelte:window {onkeydown} />

<div class="flex min-h-0 flex-1 flex-col">
  <header
    class="flex h-12 flex-none items-center gap-2 border-b border-rule px-3 max-sm:gap-1 max-sm:px-1.5"
  >
    {#if !sidebar}
      <Button variant="quiet" icon="sidebar" label="Show sidebar" onclick={onsidebar} />
    {/if}
    <div class="flex min-w-0 flex-1 items-center gap-2">
      <h1 class="truncate text-body font-semibold tracking-[-0.01em]">{title}</h1>
      {#if session?.workspace}
        <span class="hidden truncate font-mono text-small text-muted md:inline">
          <Path path={basename(session.workspace)} title={session.workspace} />
        </span>
      {/if}
    </div>
    <div class="flex flex-none items-center gap-1">
      {#if session}
        <DisplayPopover {hiddenHooks} />
      {/if}
      <span class="flex flex-none items-center gap-1.5 px-1 text-small text-muted max-sm:hidden">
        {#if stream.connection === "open"}
          <span class="inline-flex items-center gap-1.5 font-medium text-live-ink">
            <Dot tone="open" size={6} />Live
          </span>
          <span aria-hidden="true" class="text-control">·</span>
          <span class="flex-none">{machine.label}</span>
        {:else}
          <Badge size="chrome">{machine.state.text}</Badge>
        {/if}
      </span>
      {#if session && status}
        {@const tone = stream.status.alert ? "alert" : stream.status.running ? "running" : "idle"}
        <Badge
          variant={tone === "alert" ? "destructive" : tone === "running" ? "working" : "quiet"}
          size="chrome"
          class="max-w-52 min-w-0 flex-initial max-sm:hidden"
          title={status}
        >
          <Dot {tone} /><span class="truncate">{status}</span>
        </Badge>
      {/if}
      <Separator orientation="vertical" class="mx-0.5 h-4 bg-rule max-sm:hidden" />
      <Button
        variant="quiet"
        icon="agents"
        label="Agents ({stream.roster.agents.length})"
        title="Agents"
        tooltip
        aria-pressed={panel === "agents"}
        onclick={() => (panel = panel === "agents" ? undefined : "agents")}
      >
        <span class="text-small font-medium tabular-nums">{stream.roster.agents.length}</span>
      </Button>
      <Button
        variant="quiet"
        icon="diff"
        label="Changes"
        title="Changes"
        tooltip
        aria-pressed={panel === "changes"}
        onclick={() => (panel = panel === "changes" ? undefined : "changes")}
      >
        {#if net && (net.Added || net.Removed)}
          <span class="font-mono text-small font-medium tabular-nums max-sm:hidden">
            <span class="text-positive">+{net.Added}</span>
            <span class="text-negative">−{net.Removed}</span>
          </span>
        {:else if stream.review?.fileCount}
          <span class="text-small font-medium tabular-nums max-sm:hidden"
            >{stream.review.fileCount}</span
          >
        {/if}
      </Button>
      {#if selectedEntry}
        <Button
          variant="quiet"
          size="sm"
          aria-pressed={panel === "details"}
          onclick={() => (panel = panel === "details" ? undefined : "details")}
        >
          Details
        </Button>
      {/if}
      {#if session}
        <Popover label="Session details" icon="info">
          <SessionInfo {session} {root} {status} alert={stream.status.alert} />
          <div class="flex items-center justify-between gap-2 border-t border-rule px-4 py-2.5">
            <Button
              size="sm"
              variant="quiet"
              onclick={() => {
                renameValue = title;
                renameError = undefined;
                renameOpen = true;
              }}>Rename</Button
            >
            <Button size="sm" variant="danger" onclick={() => onend({ machine, id, title })}
              >End thread</Button
            >
          </div>
        </Popover>
      {/if}
    </div>
  </header>

  {#if stream.gone}
    <div class="grid flex-1 place-content-center p-6 text-center">
      <Alert>{stream.gone.text}</Alert>
    </div>
  {:else}
    <div class="flex min-h-0 flex-1">
      <div class="flex min-h-0 min-w-0 flex-1 flex-col">
        {#if stream.error}
          <div class="mx-auto w-full max-w-column px-4 pt-3 max-sm:px-2">
            <Alert>{stream.error}</Alert>
          </div>
        {/if}
        <Transcript
          entries={mainEntries}
          owner={view.agent ?? "/root"}
          workspace={session?.workspace}
          jumps={stream.jumps}
          onhiddenhooks={(count) => (hiddenHooks = count)}
          onchanges={hasChanges ? openChanges : undefined}
          onselect={openDetails}
        />
        <ThreadDocks />
        {#if hasPrompts}
          <div class="mx-auto grid w-full max-w-column gap-2 px-4 pt-2 max-sm:px-2">
            {#each semantic.prompts.approvals.slice(0, 1) as approval (approval.id)}
              <Prompt
                item={approval}
                count={semantic.prompts.approvals.length}
                unavailable={stream.connection !== "open"}
              />
            {/each}
            {#if currentQuestion}
              <Prompt
                item={currentQuestion.call}
                questionIndex={currentQuestion.index}
                position={questionPosition}
                total={questionChannel.length}
                onquestion={(index) => {
                  const target = currentQuestion.call.questions[index];
                  if (target) selectedQuestion = target.id;
                }}
                onprevious={() => {
                  const prev = questionChannel[questionPosition - 1];
                  if (prev) selectedQuestion = prev.question.id;
                }}
                onnext={() => {
                  const next = questionChannel[questionPosition + 1];
                  if (next) selectedQuestion = next.question.id;
                }}
                unavailable={stream.connection !== "open"}
              />
            {/if}
          </div>
        {/if}
        <Composer
          {machine}
          thread={id}
          input={stream.input}
          agent={view.agent ?? "/root"}
          role={root?.role}
          {initial}
          {oninitial}
          asking={hasPrompts}
          unavailable={stream.connection !== "open"
            ? machine.error || machine.state.text
            : undefined}
        />
      </div>

      {#if panel}
        {#snippet panelContent()}
          {#if panel === "agents"}
            <div class="h-full overflow-y-auto px-3 pb-4">
              <Roster
                roster={stream.roster}
                selected={view.agent ?? "/root"}
                now={wall}
                loading={stream.connection === "connecting"}
                activity={(agent) => agent.activity ?? undefined}
                onselect={(name) => show(name)}
              />
            </div>
          {:else if panel === "changes"}
            <DiffPanel
              review={stream.review}
              focusPath={focusChangePath}
              onfocused={() => (focusChangePath = undefined)}
            />
          {:else if panel === "details" && selectedEntry}
            <DetailsPanel
              entry={selectedEntry}
              onclose={() => {
                panel = undefined;
                selectedEntry = undefined;
              }}
            />
          {/if}
        {/snippet}
        {#if narrow}
          <Dialog
            title={panel === "agents" ? "Agents" : panel === "changes" ? "Changes" : "Details"}
            onclose={() => (panel = undefined)}
          >
            <div class="max-h-[70vh] overflow-y-auto">
              {@render panelContent()}
            </div>
          </Dialog>
        {:else}
          <aside
            class={[
              "flex-none border-l border-rule bg-surface",
              panel === "agents"
                ? "w-side"
                : panel === "details"
                  ? "w-[min(480px,40vw)]"
                  : "w-[min(640px,46vw)]",
            ]}
          >
            {@render panelContent()}
          </aside>
        {/if}
      {/if}
    </div>
  {/if}
</div>

{#if renameOpen}
  <Dialog title="Rename thread" onclose={() => (renameOpen = false)}>
    <form
      onsubmit={(event) => {
        event.preventDefault();
        void saveRename();
      }}
    >
      <Field label="Thread name">
        <input class={inputStyle} bind:value={renameValue} required autocomplete="off" />
      </Field>
      {#if renameError}<Alert class="mt-2">{renameError}</Alert>{/if}
      <div class="mt-4 flex justify-end gap-2">
        <Button variant="quiet" onclick={() => (renameOpen = false)}>Cancel</Button>
        <Button type="submit" variant="primary" disabled={savingName || !renameValue.trim()}>
          {savingName ? "Saving…" : "Save"}
        </Button>
      </div>
    </form>
  </Dialog>
{/if}
