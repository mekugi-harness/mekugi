<script lang="ts">
  import Alert from "./ui/Alert.svelte";
  import Code from "./Code.svelte";
  import Markdown from "./Markdown.svelte";
  import type { Entry, ThreadItem } from "./protocol";
  import { stream } from "./stream.svelte";
  import { errorText } from "./transport";
  import Button from "./ui/Button.svelte";
  import { codeBlock } from "./ui/styles";

  let {
    entry,
    onclose,
  }: {
    entry: Entry;
    onclose: () => void;
  } = $props();

  let item = $state.raw<ThreadItem | undefined>();
  let loading = $state(false);
  let error = $state<string | undefined>();

  $effect(() => {
    const ref = entry.content;
    item = undefined;
    error = undefined;
    if (!ref?.itemId) return;
    const controller = new AbortController();
    loading = true;
    void stream
      .readContent(ref, controller.signal)
      .then((res) => {
        if (!controller.signal.aborted) item = res.item;
      })
      .catch((failure) => {
        if (!controller.signal.aborted) error = errorText(failure);
      })
      .finally(() => {
        if (!controller.signal.aborted) loading = false;
      });
    return () => controller.abort();
  });
</script>

<div class="flex h-full min-h-0 flex-col">
  <div class="flex items-center justify-between gap-2 border-b border-rule px-3 py-2">
    <h2 class="truncate font-semibold">{entry.kind ?? "Details"}</h2>
    <Button variant="quiet" icon="close" label="Close details" onclick={onclose} />
  </div>
  <div class="min-h-0 flex-1 overflow-y-auto px-3 py-3 text-small">
    {#if error}
      <Alert>{error}</Alert>
    {:else if loading}
      <p class="text-muted" role="status">Loading item details…</p>
    {:else if item}
      {#if item.type === "commandExecution"}
        <p class="mb-2 font-mono text-code text-ink">{item.command}</p>
        {#if item.aggregatedOutput}
          <div class={codeBlock}>
            <Code code={item.aggregatedOutput} />
          </div>
        {/if}
      {:else if item.type === "fileChange"}
        {#each item.changes as change (change.path)}
          <div class="mb-3">
            <p class="mb-1 font-mono text-small font-medium">{change.path}</p>
            {#if change.diff}
              <div class={codeBlock}>
                <Code code={change.diff} lang="diff" />
              </div>
            {/if}
          </div>
        {/each}
      {:else if item.type === "agentMessage" || item.type === "plan"}
        <Markdown text={item.text} />
      {:else}
        <div class={codeBlock}>
          <Code code={JSON.stringify(item, null, 2)} lang="json" />
        </div>
      {/if}
    {:else if entry.text}
      <Markdown text={entry.text} />
    {:else}
      <p class="text-muted">No additional details for this item.</p>
    {/if}
  </div>
</div>
