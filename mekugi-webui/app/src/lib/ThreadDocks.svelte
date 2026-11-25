<script lang="ts">
  import Alert from "./ui/Alert.svelte";
  import type { QueuedSubmission } from "./protocol";
  import { semantic } from "./semantic.svelte";

  const queued = $derived(semantic.queued);
  const error = $derived(semantic.error);
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
        <ul class="mt-1 grid max-h-36 gap-1 overflow-y-auto">
          {#each queued as item (item.id)}
            <li class="truncate text-ink">{preview(item)}</li>
          {/each}
        </ul>
      </div>
    {/if}
  </div>
{/if}
