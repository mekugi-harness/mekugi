<script lang="ts">
  import { tick } from "svelte";
  import EntryView from "./EntryView.svelte";
  import type { Entry } from "./protocol";
  import { stream } from "./stream.svelte";
  import Button from "./ui/Button.svelte";
  import { segmentWorkdirRows, workdirGroups } from "./workdir";

  let {
    entries,
    owner = "/root",
    workspace,
    jumps = 0,
    empty = "No messages yet.",
    onselect,
  }: {
    entries: Entry[];
    owner?: string;
    workspace?: string;
    start?: number;
    span?: number;
    jumps?: number;
    empty?: string;
    onselect?: (entry: Entry) => void;
  } = $props();

  let scroller = $state<HTMLElement>();
  let pinned = $state(true);

  const segments = $derived.by(() => {
    const rows = entries.map((entry) => ({ entry }));
    const groups = workdirGroups(rows, workspace);
    return segmentWorkdirRows(rows, groups);
  });

  function onscroll() {
    if (!scroller) return;
    const distance = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    pinned = distance < 48;
  }

  $effect(() => {
    void entries.length;
    void jumps;
    if (!pinned || !scroller) return;
    void tick().then(() => {
      if (scroller) scroller.scrollTop = scroller.scrollHeight;
    });
  });

  function jumpToLatest() {
    pinned = true;
    if (scroller) scroller.scrollTop = scroller.scrollHeight;
  }
</script>

<div class="relative min-h-0 flex-1">
  <div bind:this={scroller} {onscroll} class="h-full overflow-y-auto px-4 pt-4 pb-6 max-sm:px-2">
    <div class="mx-auto flex w-full max-w-column flex-col gap-3">
      {#if segments.length}
        {#each segments as seg (seg.key)}
          {#if seg.kind === "group"}
            <section data-workdir-group={seg.workdir} class="min-w-0">
              <div
                data-workdir-heading={seg.workdir}
                class="sticky top-0 z-10 mb-1.5 flex items-center gap-1.5 bg-ground/95 py-1 font-mono text-tiny text-muted backdrop-blur-xs"
              >
                <span class="truncate">{seg.workdir}</span>
              </div>
              <div class="flex flex-col gap-3">
                {#each seg.rows as row (row.entry.seq)}
                  <div
                    class="min-w-0"
                    onclick={() => {
                      if (row.entry.content?.itemId) onselect?.(row.entry);
                    }}
                    role="presentation"
                  >
                    <EntryView
                      entry={row.entry}
                      at=""
                      stamp=""
                      {owner}
                      {workspace}
                      groupWorkdir={seg.workdir}
                    />
                  </div>
                {/each}
              </div>
            </section>
          {:else}
            <div
              class="min-w-0"
              onclick={() => {
                if (seg.row.entry.content?.itemId) onselect?.(seg.row.entry);
              }}
              role="presentation"
            >
              <EntryView entry={seg.row.entry} at="" stamp="" {owner} {workspace} />
            </div>
          {/if}
        {/each}
      {:else if stream.connection === "connecting"}
        <p class="py-8 text-center text-small text-muted" role="status">Loading thread…</p>
      {:else}
        <p class="py-8 text-center text-small text-muted">{empty}</p>
      {/if}
    </div>
  </div>
  {#if !pinned}
    <div class="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center">
      <div class="pointer-events-auto">
        <Button size="sm" variant="primary" onclick={jumpToLatest}>Jump to latest</Button>
      </div>
    </div>
  {/if}
</div>
