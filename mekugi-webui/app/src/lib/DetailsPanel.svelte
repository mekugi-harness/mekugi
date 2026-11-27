<script lang="ts">
  import { tick } from "svelte";
  import { textUploadFrames } from "./attachments";
  import Alert from "./ui/Alert.svelte";
  import { commandValues } from "./command";
  import { normalizeWorkdir } from "./workdir";
  import Code from "./Code.svelte";
  import { detailRange, type DetailLink, type LineRange } from "./detail-link";
  import { pathLanguage } from "./highlight";
  import { duration } from "./format";
  import type { Machine } from "./hub.svelte";
  import Markdown from "./Markdown.svelte";
  import type { Entry, ThreadItem } from "./protocol";
  import ReferenceSelection from "./ReferenceSelection.svelte";
  import { mcpToolEnvelope, stream } from "./stream.svelte";
  import { errorText } from "./transport";
  import Button from "./ui/Button.svelte";
  import { Skeleton } from "./ui/skeleton";
  import { codeBlock } from "./ui/styles";

  let {
    entry,
    link,
    machine,
    thread,
    onclose,
    onreference,
  }: {
    entry?: Entry;
    link?: DetailLink;
    machine: Machine;
    thread: string;
    onclose: () => void;
    onreference?: (text: string, description: string, source: string) => boolean;
  } = $props();

  let item = $state.raw<ThreadItem | undefined>();
  let file = $state<string>();
  let loading = $state(false);
  let error = $state<string | undefined>();
  let content = $state<HTMLDivElement>();
  let mark = $state<HTMLElement>();

  $effect(() => {
    const target = link;
    const owner = machine;
    const id = thread;
    const ref = entry?.content;
    item = undefined;
    file = undefined;
    error = undefined;
    loading = false;
    if (!target && !ref?.itemId) return;
    const controller = new AbortController();
    loading = true;
    const signal = controller.signal;
    const request =
      target?.kind === "file"
        ? owner.resource<{ text: string }>(
            "file",
            { thread: id, body: { path: target.path } },
            { signal },
          )
        : stream.readContent(target?.kind === "run" ? { itemId: target.itemId } : ref!, signal);
    void request
      .then((res) => {
        if (signal.aborted) return;
        if ("text" in res) file = res.text;
        else if (target?.kind !== "run" || res.item?.type === "commandExecution") item = res.item;
        else error = "The referenced command output is unavailable.";
      })
      .catch((failure) => {
        if (!signal.aborted) error = errorText(failure);
      })
      .finally(() => {
        if (!controller.signal.aborted) loading = false;
      });
    return () => controller.abort();
  });

  $effect(() => {
    const target = mark;
    if (target)
      void tick().then(() => {
        if (target.isConnected) target.scrollIntoView({ block: "center", inline: "nearest" });
      });
  });
</script>

{#snippet source(code: string, lang?: string, range?: LineRange, command = false)}
  {@const excerpt = range && detailRange(code, range, command)}
  {#if range && !excerpt}
    <Alert class="mb-2">The referenced {command ? "output" : "file"} lines are unavailable.</Alert>
  {/if}
  {#if excerpt}
    <pre class={codeBlock}><code class="block w-max min-w-full"
        >{#if excerpt.before}<span class="block min-h-lh px-2"
          ><Code code={excerpt.before.replace(/\r?\n$/, "")} {lang} /></span
        >{/if}<mark
          bind:this={mark}
          class="code-range block min-h-lh bg-active px-2 text-ink shadow-[inset_2px_0_var(--accent)]"
          aria-label={`Lines ${range?.first}–${range?.last}`}
          ><Code code={excerpt.marked} {lang} /></mark
        >{#if excerpt.after}<span class="block min-h-lh px-2"
          ><Code code={excerpt.after.replace(/^\r?\n/, "")} {lang} /></span
        >{/if}</code
      ></pre>
  {:else if code}
    <div class={codeBlock}><Code {code} {lang} /></div>
  {/if}
{/snippet}

<div id="activity-details" class="flex h-full min-h-0 flex-col">
  <div class="flex min-h-9 items-center justify-between gap-2 bg-raised/50 pr-1.5 pl-4">
    <h2
      class="truncate text-small font-medium text-muted"
      title={link?.kind === "file" ? link.label : undefined}
    >
      {link?.kind === "file"
        ? link.label
        : link?.kind === "run"
          ? "Command output"
          : (entry?.kind ?? "Details")}
    </h2>
    <Button variant="quiet" size="sm" icon="close" label="Close details" onclick={onclose} />
  </div>
  {#if onreference}
    <ReferenceSelection
      root={content}
      description="dialog"
      source={link?.kind === "file" ? link.path : ""}
      {onreference}
    />
  {/if}
  <div bind:this={content} class="min-h-0 flex-1 overflow-y-auto px-3 py-3 text-small">
    {#if error}
      <Alert>{error}</Alert>
    {:else if loading}
      <div class="grid gap-2" role="status" aria-busy="true" aria-label="Loading item details">
        <Skeleton class="h-4 w-3/4 bg-active" />
        <Skeleton class="h-24 w-full bg-active" />
      </div>
    {:else if file !== undefined && link?.kind === "file"}
      {@render source(file, pathLanguage(link.path), link.range)}
    {:else if item}
      {#if item.type === "commandExecution"}
        {@const values = commandValues(item)}
        {@const result = values[values.length - 1]}
        {@const cwd = normalizeWorkdir(result.workdir, stream.session?.workspace)}
        {@render source(item.display?.script ?? item.command, "bash")}
        <p class="my-2 text-muted">
          {#if result.batchExit}Batch ·
          {/if}
          {result.running ? "Running" : result.failed ? "Failed" : "Completed"}
          {#if result.exitCode != null}
            · exit {result.exitCode}{/if}
          {#if result.duration != null}
            · {duration(result.duration)}{/if}
          {#if cwd}
            · {cwd}{/if}
        </p>
        {@render source(item.aggregatedOutput ?? "", undefined, link?.range, true)}
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
      {:else if item.type === "mcpToolCall"}
        <p class="mb-2 font-mono text-ink">{item.server}:{item.tool}</p>
        {@render source(mcpToolEnvelope(item), "json")}
      {:else if item.type === "contextCompaction"}
        {@const reset = item.summaryFree || item.sourceAcquisition?.purpose === "reload"}
        <h3 class="mb-2 font-medium text-ink">{reset ? "Context reset" : "Context compacted"}</h3>
        {#if item.startedAtMs != null && item.completedAtMs != null}
          <p class="mb-2 text-muted">Took {duration(item.completedAtMs - item.startedAtMs)}</p>
        {/if}
        {#if item.summary}
          <Markdown text={item.summary} />
        {:else}
          <p class="text-muted">
            {reset ? "No summary was generated." : "No plaintext summary was retained."}
          </p>
        {/if}
        {#if item.sourceAcquisition?.sources.length}
          <h4 class="mt-3 mb-1 font-medium">Reloaded sources</h4>
          <ul class="grid gap-1">
            {#each item.sourceAcquisition.sources as source}
              <li>
                <code class="wrap-anywhere">{source.reference}</code>
                {#if !source.success}
                  <span class="text-muted">{source.diagnostic ?? "Unavailable"}</span>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      {:else if item.type === "userMessage"}
        {#each item.content as part}
          {#if part.type === "text"}
            {@const frames = textUploadFrames(part.text)}
            {#if frames.length}
              {#each frames as frame}
                {@render source(frame)}
              {/each}
            {:else}
              <Markdown text={part.text} />
            {/if}
          {/if}
        {/each}
      {:else if item.type === "reasoning"}
        {#if item.summary.some((section) => section.trim())}
          <Markdown text={item.summary.filter((section) => section.trim()).join("\n\n")} />
        {:else}
          <p class="text-muted">No public summary was provided.</p>
        {/if}
      {:else}
        <div class={codeBlock}>
          <Code code={JSON.stringify(item, null, 2)} lang="json" />
        </div>
      {/if}
    {:else if entry?.text}
      <Markdown text={entry.text} />
    {:else}
      <p class="text-muted">No additional details for this item.</p>
    {/if}
  </div>
</div>
