<script lang="ts">
  import Alert from "./ui/Alert.svelte";
  import type { QueuedSubmission } from "./protocol";
  import { semantic } from "./semantic.svelte";

  const queued = $derived(semantic.queued);
  const error = $derived(semantic.error);
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
</script>

{#if queued.length || error}
  <div class="mx-auto w-full max-w-column px-4 pt-2 max-sm:px-2">
    {#if error}
      <Alert class="mb-2">{error}</Alert>
    {/if}
    {#if queued.length}
      <div class="rounded-control border border-rule bg-raised px-3 py-2 text-small">
        <p class="font-medium text-muted">Queued ({queued.length})</p>
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
