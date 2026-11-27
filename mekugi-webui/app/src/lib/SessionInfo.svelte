<script lang="ts">
  import { getAbortSignal, type Snippet } from "svelte";
  import { atLeast, cost, date, tokens, tps } from "./format";
  import type { Machine } from "./hub.svelte";
  import Path from "./Path.svelte";
  import type { Agent, Session } from "./protocol";
  import Tag from "./ui/Tag.svelte";

  let {
    session,
    root,
    status,
    machine,
    visible,
    alert = false,
    children,
  }: {
    session: Session;
    root?: Agent;
    status?: string;
    machine: Machine;
    visible: boolean;
    alert?: boolean;
    children: Snippet;
  } = $props();

  type LaunchDiagnostics = { debugPath?: string | null; providerRequests?: number };
  let launch = $state.raw<LaunchDiagnostics>();

  $effect(() => {
    launch = undefined;
    if (!visible) return;
    const signal = getAbortSignal();
    void machine
      .request<LaunchDiagnostics>("server/diagnostics", {}, { signal })
      .then((result) => {
        if (!signal.aborted) launch = result;
      })
      .catch(() => {
        // Older native servers do not expose launch diagnostics.
      });
  });

  const rows = $derived(
    (
      [
        ["Started", session.started ? date(session.started) : undefined],
        ["Turns", root?.turns !== undefined ? String(root.turns) : undefined],
        [
          "Round trips",
          root?.roundtrips != null
            ? atLeast(String(root.roundtrips), root.roundtripsPartial)
            : undefined,
        ],
        [
          "Input tokens",
          root?.inputTokens != null
            ? atLeast(tokens(root.inputTokens), root.usagePartial)
            : undefined,
        ],
        [
          "Cached input tokens",
          root?.cachedInputTokens != null
            ? atLeast(tokens(root.cachedInputTokens), root.usagePartial)
            : undefined,
        ],
        [
          "Output tokens",
          root?.outputTokens != null
            ? atLeast(tokens(root.outputTokens), root.usagePartial)
            : undefined,
        ],
        [
          "Reasoning tokens",
          root?.reasoningOutputTokens != null
            ? atLeast(tokens(root.reasoningOutputTokens), root.usagePartial)
            : undefined,
        ],
        [
          "Context",
          root?.context != null
            ? `${tokens(root.context)}${root.window ? ` of ${tokens(root.window)}` : ""}`
            : undefined,
        ],
        ["Cost", root?.cost != null ? atLeast(cost(root.cost), root.costPartial) : undefined],
        ["Throughput", root?.tps != null ? tps(root.tps) : undefined],
        ["Last response", root?.lastResponse != null ? date(root.lastResponse) : undefined],
        ["Last completed", root?.lastCompleted != null ? date(root.lastCompleted) : undefined],
      ] as [string, string | undefined][]
    ).filter(([, value]) => value !== undefined),
  );
  const id = "grid grid-cols-1 gap-0.5 text-small leading-[18px]";
  const idValue = "font-mono wrap-break-word select-all";
</script>

<div class="px-[18px] pt-4 pb-[18px]">
  <h2 class="mb-3 font-semibold">Session details</h2>
  <div class="grid grid-cols-1 gap-2.5">
    {#if session.workspace}
      <div class={id}>
        <span class="text-muted">Workspace</span><span class={idValue}
          ><Path path={session.workspace} /></span
        >
      </div>
    {/if}
    <div class={id}>
      <span class="text-muted">Thread</span><span class={idValue}>{session.thread}</span>
    </div>
    <!-- A status can be a long failure; it reads as prose, not a value column. -->
    {#if status}
      <div class={id}>
        <span class="text-muted">Status</span><span
          class={["leading-5 wrap-anywhere", alert && "text-negative"]}>{status}</span
        >
      </div>
    {/if}
  </div>
  <dl class="mt-4 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 leading-7">
    {#each rows as [label, value] (label)}
      <dt class="text-muted">{label}</dt>
      <dd class="text-right tabular-nums">{value}</dd>
    {/each}
  </dl>
  {#if root?.skills?.length}
    <div class="mt-4 text-small">
      <span class="text-muted">Skills</span>
      <ul class="mt-1.5 flex flex-wrap gap-1.5">
        {#each root.skills as skill (skill)}<li>
            <Tag mono>{skill}</Tag>
          </li>{/each}
      </ul>
    </div>
  {/if}
  {#if launch?.debugPath || launch?.providerRequests != null}
    <section class="mt-4 border-t border-rule pt-3">
      <h3 class="mb-2 font-medium">This launch</h3>
      {#if launch?.providerRequests != null}
        <dl class="mb-2 flex justify-between gap-4 text-small">
          <dt class="text-muted">Provider requests</dt>
          <dd class="tabular-nums">{launch.providerRequests}</dd>
        </dl>
      {/if}
      {#if launch?.debugPath}
        <div class={id}>
          <span class="text-muted">Debug path</span>
          <span class={idValue}><Path path={launch.debugPath} /></span>
        </div>
      {/if}
    </section>
  {/if}
  {@render children()}
</div>
