<script lang="ts">
  import DiffView from "./DiffView.svelte";
  import Path from "./Path.svelte";
  import type { SavedReview } from "./protocol";
  import { emptyBox } from "./ui/styles";

  let {
    review,
  }: {
    review: SavedReview | null;
  } = $props();

  let wrap = $state(false);
  const files = $derived(review?.files ?? []);
  const net = $derived(review?.counts.Net);
</script>

<div class="flex h-full min-h-0 flex-col">
  <div class="flex items-center justify-between gap-2 border-b border-rule px-3 py-2 text-small">
    <span class="font-semibold">
      Changes {#if files.length}({files.length}){/if}
    </span>
    {#if net}
      <span class="tabular-nums">
        <span class="text-positive">+{net.Added}</span>
        <span class="text-negative">−{net.Removed}</span>
      </span>
    {/if}
  </div>
  <div class="min-h-0 flex-1 overflow-y-auto px-3 py-3">
    {#if files.length}
      <ul class="grid gap-3">
        {#each files as file (file.key)}
          <li class="rounded-card border border-rule bg-surface">
            <div
              class="flex items-center justify-between gap-2 border-b border-rule px-3 py-2 text-small"
            >
              <span class="min-w-0 truncate font-mono font-medium"><Path path={file.path} /></span>
              <span class="flex-none tabular-nums">
                <span class="text-positive">+{file.counts.Added}</span>
                <span class="text-negative">−{file.counts.Removed}</span>
              </span>
            </div>
            {#each file.chunks as chunk (chunk.Key)}
              <div class="p-2">
                <DiffView file={chunk.Review} workspace={review?.workspace ?? ""} {wrap} />
              </div>
            {/each}
          </li>
        {/each}
      </ul>
    {:else}
      <p class={emptyBox}>No file changes recorded for this thread yet.</p>
    {/if}
  </div>
</div>
