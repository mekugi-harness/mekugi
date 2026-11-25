<script lang="ts">
  import { untrack } from "svelte";
  import type { InitialInput } from "./attachments";
  import Composer from "./Composer.svelte";
  import DetailsPanel from "./DetailsPanel.svelte";
  import DiffPanel from "./DiffPanel.svelte";
  import { basename, statusText } from "./format";
  import type { Machine } from "./hub.svelte";
  import Path from "./Path.svelte";
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
  import { input as inputStyle, segment, segments } from "./ui/styles";
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
  let renameOpen = $state(false);
  let renameValue = $state("");
  let savingName = $state(false);
  let renameError = $state<string | undefined>();
  let wall = $state(Date.now());

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

  function openDetails(entry: Entry) {
    selectedEntry = entry;
    panel = "details";
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

<div class="flex min-h-0 flex-1 flex-col">
  <header class="flex h-12 flex-none items-center gap-2 border-b border-rule px-3">
    {#if !sidebar}
      <Button variant="quiet" icon="sidebar" label="Show sidebar" onclick={onsidebar} />
    {/if}
    <div class="flex min-w-0 flex-1 items-center gap-2">
      <Dot
        tone={stream.status.alert ? "alert" : stream.status.running ? "running" : "none"}
        size={7}
      />
      <h1 class="truncate font-semibold">{title}</h1>
      {#if session?.workspace}
        <span class="hidden truncate font-mono text-small text-muted md:inline">
          <Path path={basename(session.workspace)} title={session.workspace} />
        </span>
      {/if}
      {#if stream.connection !== "open"}
        <Badge size="chrome">{machine.state.text}</Badge>
      {/if}
    </div>
    <div class="flex flex-none items-center gap-1.5">
      <div class={segments} role="group" aria-label="Side panels">
        <button
          type="button"
          class={segment}
          aria-pressed={panel === "agents"}
          onclick={() => (panel = panel === "agents" ? undefined : "agents")}
        >
          Agents
        </button>
        <button
          type="button"
          class={segment}
          aria-pressed={panel === "changes"}
          onclick={() => (panel = panel === "changes" ? undefined : "changes")}
        >
          Changes {#if stream.review?.fileCount}({stream.review.fileCount}){/if}
        </button>
        {#if selectedEntry}
          <button
            type="button"
            class={segment}
            aria-pressed={panel === "details"}
            onclick={() => (panel = panel === "details" ? undefined : "details")}
          >
            Details
          </button>
        {/if}
      </div>
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
            <DiffPanel review={stream.review} />
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
        <aside class="w-80 flex-none border-l border-rule bg-ground max-lg:hidden">
          {@render panelContent()}
        </aside>
        <div
          class="fixed inset-0 z-40 flex justify-end lg:hidden"
          role="dialog"
          aria-modal="true"
          aria-label={panel === "agents" ? "Agents" : panel === "changes" ? "Changes" : "Details"}
        >
          <button
            type="button"
            class="flex-1 bg-black/40"
            aria-label="Close panel"
            onclick={() => (panel = undefined)}
          ></button>
          <div
            class="flex h-full w-[min(22rem,90vw)] flex-col border-l border-rule bg-ground shadow-lg"
          >
            <header
              class="flex flex-none items-center justify-between border-b border-rule px-3 py-2"
            >
              <h2 class="text-small font-medium">
                {panel === "agents" ? "Agents" : panel === "changes" ? "Changes" : "Details"}
              </h2>
              <Button
                variant="quiet"
                size="sm"
                icon="close"
                label="Close panel"
                onclick={() => (panel = undefined)}
              />
            </header>
            <div class="min-h-0 flex-1 overflow-y-auto">
              {@render panelContent()}
            </div>
          </div>
        </div>
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
