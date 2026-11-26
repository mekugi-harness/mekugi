<script lang="ts">
  import { tick } from "svelte";
  import { preferredScrollBehavior } from "./diff";
  import EntryView from "./EntryView.svelte";
  import { date } from "./format";
  import {
    editEntryPaths,
    failed,
    hasDetails,
    isEditEntry,
    isUser,
    isView,
    responseText,
  } from "./kinds";
  import Path from "./Path.svelte";
  import { preferences } from "./preferences.svelte";
  import ResponseCopy from "./ResponseCopy.svelte";
  import type { Entry } from "./protocol";
  import { stream } from "./stream.svelte";
  import Icon from "./ui/Icon.svelte";
  import { Skeleton } from "./ui/skeleton";
  import { emptyBox, revealed, rowAction } from "./ui/styles";
  import { segmentWorkdirRows, workdirGroups } from "./workdir";

  let {
    entries,
    owner = "/root",
    workspace,
    jumps = 0,
    empty = "No messages yet.",
    onselect,
    onchanges,
    hasSavedChange,
    onhiddenhooks,
  }: {
    entries: Entry[];
    owner?: string;
    workspace?: string;
    start?: number;
    span?: number;
    jumps?: number;
    empty?: string;
    onselect?: (entry: Entry) => void;
    onchanges?: (paths: string[]) => void;
    hasSavedChange?: (paths: string[]) => boolean;
    onhiddenhooks?: (count: number) => void;
  } = $props();

  let scroller = $state<HTMLElement>();
  let contentEl = $state<HTMLElement>();
  let pinned = $state(true);

  const isHiddenHook = (entry: Entry): boolean => {
    if (entry.kind !== "hook") return false;
    if (preferences.hooks === "all") return false;
    if (preferences.hooks === "off") return true;
    return !entry.values?.some(failed);
  };

  const filtered = $derived.by(() => {
    let hiddenHooks = 0;
    const grouped: {
      entry: Entry;
      rawAt: string;
      turn?: number;
      views?: Entry[];
      thoughts?: Entry[];
    }[] = [];
    for (const entry of entries) {
      const last = grouped.at(-1);
      if (last?.thoughts && entry.kind === "reasoning" && entry.agent === last.entry.agent) {
        last.thoughts.push(entry);
        continue;
      }
      if (
        last?.views &&
        isView(entry) &&
        entry.agent === last.entry.agent &&
        entry.values?.[0]?.verb === last.entry.values?.[0]?.verb
      ) {
        last.views.push(entry);
        continue;
      }
      const rawAt = entry.observed ? date(entry.observed).slice(11) : "";
      grouped.push({
        entry,
        rawAt,
        turn: entry.turn,
        views: isView(entry) ? [entry] : undefined,
        thoughts: entry.kind === "reasoning" ? [entry] : undefined,
      });
    }
    let previous = "";
    const out: {
      entry: Entry;
      at: string;
      turn?: number;
      views?: Entry[];
      thoughts?: Entry[];
    }[] = [];
    for (const row of grouped) {
      if (isHiddenHook(row.entry)) {
        hiddenHooks++;
        continue;
      }
      let entry = row.entry;
      let rawAt = row.rawAt;
      let thoughts = row.thoughts;
      if (thoughts && preferences.thoughts === "hidden") {
        const liveThoughts: Entry[] = thoughts.flatMap((e) => {
          const liveValues = e.values?.filter((v) => v.live);
          return liveValues?.length ? [{ ...e, values: liveValues }] : [];
        });
        if (!liveThoughts.length) continue;
        thoughts = liveThoughts;
        entry = liveThoughts[0];
        rawAt = entry.observed ? date(entry.observed).slice(11) : "";
      }
      const user = isUser(entry);
      out.push({
        entry,
        at: user || rawAt !== previous ? rawAt : "",
        turn: row.turn,
        views: row.views,
        thoughts,
      });
      previous = rawAt;
    }
    return { rows: out, hiddenHooks };
  });
  const rows = $derived(filtered.rows);
  $effect(() => {
    onhiddenhooks?.(filtered.hiddenHooks);
  });

  const segments = $derived.by(() => {
    const groups = workdirGroups(rows, workspace);
    return segmentWorkdirRows(rows, groups);
  });

  function onscroll() {
    if (!scroller) return;
    const distance = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    pinned = distance < 48;
    if (scroller.scrollTop < 120 && stream.hasEarlier && !stream.loadingEarlier) {
      const prevHeight = scroller.scrollHeight;
      const prevTop = scroller.scrollTop;
      void stream.earlier().then((loaded) => {
        if (!loaded || !scroller) return;
        void tick().then(() => {
          if (!scroller) return;
          scroller.scrollTop = prevTop + (scroller.scrollHeight - prevHeight);
        });
      });
    }
  }

  $effect(() => {
    void entries;
    void jumps;
    if (!pinned || !scroller) return;
    void tick().then(() => {
      if (scroller) scroller.scrollTop = scroller.scrollHeight;
    });
  });

  $effect(() => {
    if (!scroller || !contentEl || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(() => {
      if (pinned && scroller) scroller.scrollTop = scroller.scrollHeight;
    });
    observer.observe(contentEl);
    return () => observer.disconnect();
  });

  function jumpToLatest() {
    pinned = true;
    scroller?.scrollTo({ top: scroller.scrollHeight, behavior: preferredScrollBehavior() });
  }
</script>

{#snippet transcriptRow(row: (typeof rows)[number], groupWorkdir?: string)}
  {@const entry = row.entry}
  {@const editPaths = isEditEntry(entry) ? editEntryPaths(entry) : []}
  {@const openChanges =
    !!onchanges &&
    isEditEntry(entry) &&
    (!hasSavedChange || (editPaths.length > 0 && hasSavedChange(editPaths)))}
  {@const details = !!onselect && (!!entry.content?.itemId || hasDetails(entry, owner))}
  {@const hasRowAction = openChanges || details}
  {@const response =
    !isUser(entry) && !entry.assignment
      ? responseText(entry.kind, entry.text, entry.values?.[0]?.body)
      : ""}
  <div class="group/row relative" data-entry={entry.seq}>
    <EntryView
      {entry}
      at={row.at}
      stamp="Time"
      timestampMode={preferences.timestamps}
      expandThoughts={preferences.thoughts === "expanded"}
      turn={row.turn}
      views={row.views}
      thoughts={row.thoughts}
      {owner}
      {groupWorkdir}
      {workspace}
      ondetails={openChanges
        ? () => onchanges?.(editPaths)
        : details
          ? () => onselect?.(entry)
          : undefined}
    />
    {#if openChanges}
      <button
        type="button"
        class="{rowAction} {revealed} absolute top-1 -right-9 text-muted max-sm:-right-10"
        aria-label="View changes"
        title="View changes"
        onclick={() => onchanges?.(editPaths)}
      >
        <Icon name="diff" size={14} />
      </button>
    {:else if details}
      <button
        type="button"
        class="{rowAction} {revealed} absolute top-1 -right-9 text-muted max-sm:-right-10"
        aria-label="Details"
        title="Details"
        onclick={() => onselect?.(entry)}
      >
        <Icon name="expand" size={14} />
      </button>
    {/if}
    {#if response}
      <ResponseCopy text={response} gutter below={hasRowAction} />
    {/if}
  </div>
{/snippet}

<div class="relative min-h-0 flex-1">
  <div bind:this={scroller} {onscroll} class="@container h-full overflow-y-auto overscroll-contain">
    <div
      bind:this={contentEl}
      class="mx-auto max-w-[calc(var(--container-column)+184px)] pt-3 pr-23 pb-10 pl-4 @max-[980px]:pr-11 max-sm:pt-2 max-sm:pb-6"
    >
      {#if stream.loadingEarlier}
        <div
          class="mb-4 ml-19 grid gap-2 max-sm:ml-0"
          role="status"
          aria-label="Loading earlier turns"
        >
          <Skeleton class="h-4 w-2/3 bg-active" />
          <Skeleton class="h-4 w-1/2 bg-active" />
        </div>
      {/if}
      {#if segments.length}
        {#each segments as seg (seg.key)}
          {#if seg.kind === "group"}
            <section data-workdir-group={seg.workdir} class="my-2 first:mt-0">
              <div
                data-workdir-heading={seg.workdir}
                class="sticky top-0 z-10 grid max-w-[920px] grid-cols-[60px_minmax(0,1fr)] gap-x-4 bg-surface/95 py-1.5 backdrop-blur-sm max-sm:grid-cols-1"
              >
                <span class="max-sm:hidden" aria-hidden="true"></span>
                <div
                  class="flex min-w-0 items-baseline gap-1.5 border-b border-rule/80 pb-1 text-small"
                >
                  <span class="shrink-0 text-muted">In</span>
                  <span class="min-w-0 font-mono text-ink"><Path path={seg.workdir} /></span>
                </div>
              </div>
              {#each seg.rows as row (row.entry.seq)}
                {@render transcriptRow(row, seg.workdir)}
              {/each}
            </section>
          {:else}
            {@render transcriptRow(seg.row)}
          {/if}
        {/each}
      {:else if stream.connection === "connecting"}
        <div class="mt-6 ml-19 grid gap-3 max-sm:ml-0" role="status" aria-label="Loading thread">
          <Skeleton class="h-5 w-3/4 bg-active" />
          <Skeleton class="h-5 w-1/2 bg-active" />
          <Skeleton class="h-5 w-5/6 bg-active" />
        </div>
      {:else}
        <p class="{emptyBox} mt-6 ml-17 max-sm:ml-0">{empty}</p>
      {/if}
    </div>
  </div>
  {#if !pinned}
    <button
      type="button"
      class="absolute bottom-4 left-1/2 flex h-8 -translate-x-1/2 items-center gap-1.5 rounded-full border border-border bg-surface/95 px-3.5 text-small font-medium text-ink shadow-float backdrop-blur-sm transition-colors hover:border-control hover:bg-raised max-sm:h-10"
      onclick={jumpToLatest}
    >
      <Icon name="down" size={14} /> Latest
    </button>
  {/if}
</div>
