<script lang="ts">
  import AgentName from "./AgentName.svelte";
  import Markdown from "./Markdown.svelte";
  import ProcessCommand from "./ProcessCommand.svelte";
  import Alert from "./ui/Alert.svelte";
  import Button from "./ui/Button.svelte";
  import type { BackgroundTerminal, QueuedSubmission } from "./protocol";
  import RecoveryEditor from "./RecoveryEditor.svelte";
  import ResponseCopy from "./ResponseCopy.svelte";
  import { editableRecoveryPart, semantic } from "./semantic.svelte";
  import { stream } from "./stream.svelte";

  const queued = $derived(semantic.queued);
  const error = $derived(semantic.error);
  const recoveries = $derived(semantic.recoveries);
  const answer = $derived(semantic.sideAnswer);
  const hasWaiting = $derived(queued.some((item) => item.kind === "message"));
  const disabled = $derived(semantic.connection !== "open" || semantic.pending > 0);
  const processes = $derived(stream.processes);
  let processError = $state<string>();
  let stopping = $state.raw(new Set<string>());
  let queueScroller = $state<HTMLDivElement>();
  let queueScrollable = $state(false);

  $effect(() => {
    const node = queueScroller;
    void queued.length;
    if (!node) {
      queueScrollable = false;
      return;
    }
    const measure = () => {
      queueScrollable = node.scrollHeight > node.clientHeight;
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    if (node.firstElementChild) observer.observe(node.firstElementChild);
    return () => observer.disconnect();
  });

  const preview = (item: QueuedSubmission) =>
    item.input
      .map((part) =>
        part.type === "text" ? part.text : part.type === "localImage" ? part.path : part.type,
      )
      .join(" ")
      .trim() || item.id;

  const processKey = ({ threadId, turnId, itemId }: BackgroundTerminal) =>
    JSON.stringify([threadId, turnId, itemId]);

  async function stopProcess(process: BackgroundTerminal) {
    const key = processKey(process);
    stopping = new Set(stopping).add(key);
    processError = undefined;
    try {
      const failure = await stream.stopProcess(process);
      if (failure) processError = failure;
    } finally {
      stopping = new Set([...stopping].filter((pending) => pending !== key));
    }
  }
</script>

{#if processes.length || processError || queued.length || recoveries.length || error || answer?.exchanges.length}
  <div class="mx-auto w-full max-w-column px-4 pt-2 max-sm:px-2">
    {#if error}
      <Alert class="mb-2">{error}</Alert>
    {/if}
    {#if answer?.exchanges.length}
      <section
        class="mb-2 rounded-float bg-raised px-3.5 py-2.5 shadow-xs"
        aria-label="Side answer"
      >
        <header class="mb-1 flex items-center justify-between">
          <h2 class="text-small font-semibold text-muted">By the way</h2>
          <Button
            variant="quiet"
            size="sm"
            icon="close"
            label="Close answer"
            class="max-sm:min-h-11"
            disabled={answer.closing || semantic.connection !== "open"}
            onclick={() => void answer.close()}
          />
        </header>
        <div class="flex max-h-[24dvh] flex-col gap-2 overflow-y-auto scroll-py-1 p-1 -m-1">
          {#each answer.exchanges as exchange}
            <div class="text-muted"><Markdown text={exchange.question} /></div>
            {#each [...exchange.messages] as [id, text] (id)}
              <div class="group/entry">
                <Markdown {text} />
                {#if text.trim()}<ResponseCopy {text} />{/if}
              </div>
            {/each}
          {/each}
        </div>
        {#if answer.error}
          <Alert class="mt-1">{answer.error}</Alert>
        {:else if answer.closing || answer.busy}
          <p class="mt-1 text-small text-muted" role="status">
            {answer.closing ? "Closing…" : "Answering…"}
          </p>
        {/if}
      </section>
    {/if}
    {#if processError}
      <Alert class="mb-2">{processError}</Alert>
    {/if}
    {#if processes.length}
      <section
        class="mb-2 rounded-float bg-raised px-3.5 py-2.5 shadow-xs"
        aria-label="Active processes"
      >
        <h2 class="mb-1 text-small font-semibold text-muted">Active processes</h2>
        <ul class="grid max-h-36 gap-1 overflow-y-auto">
          {#each processes as process (processKey(process))}
            {@const owner = stream.roster.agents.find((agent) => agent.thread === process.threadId)}
            <li class="flex min-w-0 items-center gap-2">
              <div class="min-w-0 flex-1"><ProcessCommand command={process.command} /></div>
              {#if owner && owner.name !== "/root"}
                <span class="min-w-0 max-w-[30%] truncate text-small text-muted">
                  <AgentName name={owner.name} current="" quiet />
                </span>
              {/if}
              <Button
                variant="quiet"
                size="sm"
                class="max-sm:min-h-11"
                disabled={semantic.connection !== "open" || stopping.has(processKey(process))}
                label={`Stop ${process.command}`}
                onclick={() => void stopProcess(process)}>Stop</Button
              >
            </li>
          {/each}
        </ul>
      </section>
    {/if}
    {#if recoveries.length}
      <div
        class="mb-2 grid max-h-[28dvh] gap-2 overflow-y-auto"
        aria-label="Draft recovery"
        role="region"
      >
        {#each recoveries as draft (draft.id)}
          <section
            class="rounded-float bg-raised px-3.5 py-2.5 text-small shadow-xs"
            aria-label="Unsent submission"
          >
            <h2 class="font-medium text-muted">
              {draft.shell !== undefined
                ? "Unsent shell command"
                : draft.error
                  ? "Unsent submission"
                  : "Edit queued message"}
            </h2>
            {#if draft.error}<Alert class="my-1">{draft.error}</Alert>{/if}
            {#if draft.editing}
              <RecoveryEditor {draft} {disabled} />
            {:else}
              <p
                class={[
                  "whitespace-pre-wrap wrap-anywhere",
                  draft.shell !== undefined && "font-mono",
                ]}
              >
                {draft.shell ??
                  (draft.input
                    .filter(editableRecoveryPart)
                    .map((part) => part.text)
                    .join("\n") ||
                    "Attachments")}
              </p>
              <div class="mt-1 flex flex-wrap gap-1">
                <Button
                  size="sm"
                  variant="quiet"
                  class="max-sm:min-h-11"
                  {disabled}
                  onclick={() => semantic.recover(draft.id, "edit")}>Edit</Button
                >
                <Button
                  size="sm"
                  variant="quiet"
                  class="max-sm:min-h-11"
                  {disabled}
                  onclick={() => semantic.recover(draft.id, "retry")}>Retry intact</Button
                >
                <Button
                  size="sm"
                  variant="quiet"
                  class="max-sm:min-h-11"
                  disabled={semantic.pending > 0}
                  onclick={() => semantic.recover(draft.id, "discard")}>Discard</Button
                >
              </div>
            {/if}
          </section>
        {/each}
      </div>
    {/if}
    {#if queued.length}
      <div class="rounded-float bg-raised px-3.5 py-2.5 text-small shadow-xs">
        <div class="flex items-center justify-between gap-2">
          <p class="font-medium text-muted">Queued ({queued.length})</p>
          {#if hasWaiting}
            <div class="flex items-center gap-1">
              <Button
                size="sm"
                variant="quiet"
                class="max-sm:min-h-11"
                {disabled}
                label="Edit queued messages"
                onclick={() => semantic.editWaiting()}>Edit</Button
              >
              <Button
                size="sm"
                variant="quiet"
                class="max-sm:min-h-11"
                disabled={disabled || !stream.activeTurnId}
                label="Steer queued messages"
                onclick={() => {
                  if (stream.activeTurnId) void semantic.steerWaiting(stream.activeTurnId);
                }}>Steer</Button
              >
            </div>
          {/if}
        </div>
        <!-- svelte-ignore a11y_no_noninteractive_tabindex (Keyboard scrolling.) -->
        <div
          bind:this={queueScroller}
          tabindex={queueScrollable ? 0 : undefined}
          role="region"
          aria-label="Queued messages"
          class="mt-1 max-h-36 overflow-y-auto"
        >
          <ul class="grid gap-1">
            {#each queued as item (item.id)}
              <li class="truncate text-ink">{preview(item)}</li>
            {/each}
          </ul>
        </div>
      </div>
    {/if}
  </div>
{/if}
