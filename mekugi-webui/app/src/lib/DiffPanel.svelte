<script lang="ts">
  import { tick, untrack } from "svelte";
  import type { Attachment } from "svelte/attachments";
  import { matchReviewedFile, preferredScrollBehavior, workspacePath } from "./diff";
  import DiffView from "./DiffView.svelte";
  import Path from "./Path.svelte";
  import type { ReviewedFile, SavedReview } from "./protocol";
  import Button from "./ui/Button.svelte";
  import { Skeleton } from "./ui/skeleton";
  import { caret, emptyBox } from "./ui/styles";

  let {
    review,
    focusPath,
    onfocused,
  }: {
    review: SavedReview | null;
    focusPath?: string;
    onfocused?: () => void;
  } = $props();

  const wrap = false;
  const files = $derived(review?.files ?? []);
  const net = $derived(review?.counts.Net);

  // Every file starts expanded by default; an IntersectionObserver preloads
  // diff views as the reader scrolls near them so large reviews stay snappy.
  let toggled = $state<Record<string, boolean>>({});
  let nearViewport = $state<Record<string, boolean>>({});
  let scroller = $state<HTMLElement>();
  const open = (file: ReviewedFile) => toggled[file.key] ?? true;
  const anyOpen = $derived(files.some((file) => open(file)));

  function setAllOpen(nextOpen: boolean) {
    const nextState: Record<string, boolean> = {};
    for (const file of files) nextState[file.key] = nextOpen;
    toggled = nextState;
  }

  let observer: IntersectionObserver | undefined;
  const observedNodes = new Map<HTMLElement, string>();
  $effect(() => {
    const root = scroller ?? null;
    const io = new IntersectionObserver(
      (entries) => {
        let changed = false;
        const nextNear = { ...nearViewport };
        for (const entry of entries) {
          if (!entry.isIntersecting) continue;
          const key = (entry.target as HTMLElement).dataset.file;
          if (key && !nextNear[key]) {
            nextNear[key] = true;
            changed = true;
          }
        }
        if (changed) nearViewport = nextNear;
      },
      { root, rootMargin: "240px 0px" },
    );
    observer = io;
    for (const node of observedNodes.keys()) io.observe(node);
    return () => {
      io.disconnect();
      if (observer === io) observer = undefined;
    };
  });

  const observeFile =
    (key: string): Attachment<HTMLElement> =>
    (node) => {
      observedNodes.set(node, key);
      observer?.observe(node);
      return () => {
        observedNodes.delete(node);
        observer?.unobserve(node);
      };
    };

  $effect(() => {
    const target = focusPath;
    if (!target || !files.length || !scroller) return;
    const match = matchReviewedFile(files, target, review?.workspace ?? "");
    if (!match) return;
    untrack(() => {
      toggled[match.key] = true;
      nearViewport = { ...nearViewport, [match.key]: true };
      void tick().then(() => {
        const el = [...scroller!.querySelectorAll<HTMLElement>("[data-file]")].find(
          (node) => node.dataset.file === match.key,
        );
        el?.scrollIntoView({ block: "start", behavior: preferredScrollBehavior() });
        onfocused?.();
      });
    });
  });
</script>

<div class="flex h-full min-h-0 flex-col">
  <div class="flex min-h-11 items-center gap-2 border-b border-rule px-4 py-2 text-small">
    <span class="flex items-center gap-2 font-semibold">
      <span
        >Changes {#if files.length}<span class="font-normal text-muted tabular-nums"
            >({files.length})</span
          >{/if}</span
      >
      {#if review?.coverage === "partial"}
        <span
          class="rounded-chip border border-warning-edge bg-warning-tint px-1.5 py-0.5 text-tiny font-medium text-warning-ink"
          title="One or more file changes have partial write coverage or missing baselines"
        >
          Partial coverage
        </span>
      {/if}
    </span>
    {#if net}
      <span class="font-mono tabular-nums">
        <span class="text-positive">+{net.Added}</span>
        <span class="text-negative">−{net.Removed}</span>
      </span>
    {/if}
    {#if files.length > 1}
      <Button
        variant="quiet"
        size="sm"
        class="ml-auto text-muted"
        onclick={() => setAllOpen(!anyOpen)}
      >
        {anyOpen ? "Collapse all" : "Expand all"}
      </Button>
    {/if}
  </div>
  <div bind:this={scroller} class="min-h-0 flex-1 overflow-y-auto overscroll-contain px-4 py-3">
    {#if files.length}
      <ul class="grid gap-2">
        {#each files as file, index (file.key)}
          {@const isOpen = open(file)}
          {@const ready = nearViewport[file.key] || index < 2}
          <li data-file={file.key} {@attach observeFile(file.key)}>
            <details
              class="group/disclosure"
              open={isOpen}
              ontoggle={(e) => {
                const now = (e.currentTarget as HTMLDetailsElement).open;
                if (now !== open(file)) toggled[file.key] = now;
                if (now && !nearViewport[file.key])
                  nearViewport = { ...nearViewport, [file.key]: true };
              }}
            >
              <summary
                class="flex min-h-9 cursor-pointer items-center gap-2 rounded-control bg-raised/60 px-2.5 py-1 text-small transition-colors hover:bg-active/80 {caret}"
              >
                <span class="flex min-w-0 flex-1 items-center gap-1.5">
                  <span class="min-w-0 truncate font-mono text-code font-medium"
                    ><Path path={workspacePath(file.path, review?.workspace ?? "")} /></span
                  >
                  {#if file.external}
                    <span
                      class="flex-none rounded-chip border border-rule bg-raised px-1.5 py-0.5 text-tiny text-muted"
                      title={`Write outside workspace: ${file.path}`}
                    >
                      External
                    </span>
                  {/if}
                  {#if file.coverageNotes?.length}
                    <span
                      class="flex-none rounded-chip border border-warning-edge bg-warning-tint px-1.5 py-0.5 text-tiny text-warning-ink"
                      title={file.coverageNotes.join("; ")}
                    >
                      Partial
                    </span>
                  {/if}
                </span>
                <span class="flex-none font-mono tabular-nums">
                  <span class={file.counts.Added ? "text-positive" : "text-muted"}
                    >+{file.counts.Added}</span
                  >
                  <span class={file.counts.Removed ? "text-negative" : "text-muted"}
                    >−{file.counts.Removed}</span
                  >
                </span>
              </summary>
              {#if isOpen}
                <div class="pb-1">
                  {#if ready}
                    {#each file.chunks as chunk (chunk.Key)}
                      <DiffView file={chunk.Review} workspace={review?.workspace ?? ""} {wrap} />
                    {/each}
                  {:else}
                    <div
                      class="mt-1.5 grid gap-1.5 rounded-card bg-raised p-3"
                      aria-label="Loading diff"
                    >
                      <Skeleton class="h-3.5 w-3/4 bg-active" />
                      <Skeleton class="h-3.5 w-1/2 bg-active" />
                      <Skeleton class="h-3.5 w-5/6 bg-active" />
                    </div>
                  {/if}
                </div>
              {/if}
            </details>
          </li>
        {/each}
      </ul>
    {:else}
      <p class={emptyBox}>No file changes recorded for this thread yet.</p>
    {/if}
  </div>
</div>
