<script lang="ts">
  import type { FeatureUseCounts } from "@codex-protocol/v2/FeatureUseCounts";
  import { getAbortSignal } from "svelte";
  import type { Machine } from "./hub.svelte";
  import type { ThreadListResponse } from "./protocol";

  let { machine, thread }: { machine: Machine; thread: string } = $props();

  // Native u64 counts are numbers on JSON-RPC, not JavaScript bigint values.
  type Counts = { [K in keyof FeatureUseCounts]: number };
  type Row = [label: string, ...keys: (keyof Counts)[]];
  const groups: { title: string; rows: Row[] }[] = [
    {
      title: "Compaction",
      rows: [
        ["Total", "compactions"],
        ["Self-initiated", "selfCompactions"],
        ["With preload", "compactionPreloads"],
        ["Self completed", "selfCompactionsCompleted"],
        ["Self failed", "selfCompactionFailures"],
      ],
    },
    {
      title: "Spawns",
      rows: [
        ["Total", "spawns"],
        ["With preload", "spawnPreloads"],
        ["Preload rejected", "preloadRejections"],
      ],
    },
    {
      title: "Delegate finals",
      rows: [
        ["Completed", "finals"],
        ["With edit ranges", "completionChanges"],
      ],
    },
    {
      title: "Messages",
      rows: [
        ["Sent", "messages"],
        ["With BTW", "sideQuestions"],
        ["BTW rejected", "sideQuestionRejections"],
        ["BTW failed", "sideQuestionFailures"],
      ],
    },
    {
      title: "Waits",
      rows: [
        ["Registered", "waits"],
        ["All mode", "allWaits"],
        ["Progress enabled", "progressWaits"],
        ["Progress delivered", "progressYields"],
      ],
    },
    {
      title: "Source-bearing sends",
      rows: [
        ["Total", "assignmentSources", "messageSources"],
        ["Assignments", "assignmentSources"],
        ["Messages", "messageSources"],
      ],
    },
    {
      title: "Output deduplication",
      rows: [
        ["Saved bytes", "outputDeduplicationBytes"],
        ["Saved tokens (est.)", "outputDeduplicationTokens"],
      ],
    },
  ];

  let open = $state(false);
  let snapshot = $state.raw<{ self: Counts | undefined; descendants: Counts[] }>();
  let failure = $state<string>();

  async function read(target: Machine, id: string, signal: AbortSignal) {
    try {
      const { data } = await target.request<ThreadListResponse>(
        "thread/subtree/list",
        { threadId: id },
        { signal },
      );
      const ids = [...new Set([id, ...data.map((entry) => entry.id)])];
      const { threads } = await target.request<{ threads: Partial<Record<string, Counts>> }>(
        "featureUse/read",
        { threads: ids },
        { signal },
      );
      if (signal.aborted) return;
      snapshot = {
        self: threads[id],
        descendants: ids
          .slice(1)
          .map((child) => threads[child])
          .filter((row): row is Counts => Boolean(row)),
      };
    } catch (error) {
      if (!signal.aborted) failure = error instanceof Error ? error.message : String(error);
    }
  }

  $effect(() => {
    if (!open) return;
    snapshot = undefined;
    failure = undefined;
    void read(machine, thread, getAbortSignal());
  });

  function count(rows: Counts[], keys: (keyof Counts)[]) {
    return rows.reduce((total, row) => keys.reduce((sum, key) => sum + row[key], total), 0);
  }
</script>

<details
  class="mt-3 border-t border-rule pt-2 text-small"
  ontoggle={(event) => (open = event.currentTarget.open)}
>
  <summary class="cursor-pointer py-1 font-medium">Features</summary>
  {#if open}
    {#if snapshot}
      <p class="mt-1 mb-2 text-muted">
        Counts cover this launch. Self is this thread; Descendants sums its
        {snapshot.descendants.length} agents, including completed ones.
      </p>
      <table class="w-full table-fixed wrap-anywhere leading-6">
        <thead>
          <tr class="text-muted">
            <th class="w-1/2 text-left font-normal" scope="col">Feature</th>
            <th class="w-1/5 pr-2 text-right font-normal" scope="col">Self</th>
            <th class="text-right font-normal" scope="col">Descendants</th>
          </tr>
        </thead>
        {#each groups as { title, rows } (title)}
          <tbody>
            <tr
              ><th colspan="3" scope="rowgroup" class="border-t border-rule pt-1 text-left"
                >{title}</th
              ></tr
            >
            {#each rows as [label, ...keys] (label)}
              <tr>
                <th class="pl-2 text-left font-normal" scope="row">{label}</th>
                <td class="pr-2 text-right tabular-nums"
                  >{snapshot.self ? count([snapshot.self], keys) : "—"}</td
                >
                <td class="text-right tabular-nums">{count(snapshot.descendants, keys)}</td>
              </tr>
            {/each}
          </tbody>
        {/each}
      </table>
      <p class="mt-2 text-muted">
        Deduplication counts net model-input savings. The token estimate is not provider billing
        data.
      </p>
    {:else if failure}
      <p class="py-1 text-negative" role="status">Could not load features: {failure}</p>
    {:else}
      <p class="py-1 text-muted" role="status">Loading features…</p>
    {/if}
  {/if}
</details>
