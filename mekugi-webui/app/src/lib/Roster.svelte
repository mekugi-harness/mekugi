<script lang="ts">
  import { doing, since, stateLabel, worked } from "./agents";
  import { atLeast, cost, duration, progressText, tokens, tps } from "./format";
  import { preferences } from "./preferences.svelte";
  import type { Agent, Counts, Entry, Roster } from "./protocol";
  import ContextCard from "./ui/ai/ContextCard.svelte";
  import Dot, { type Tone } from "./ui/Dot.svelte";
  import { Skeleton } from "./ui/skeleton";
  import { emptyBox } from "./ui/styles";
  import Tag from "./ui/Tag.svelte";
  import { Progress } from "./ui/progress";
  import { stream } from "./stream.svelte";
  import Wait from "./Wait.svelte";

  // The agents of the session, as a tree under Main. Selecting one shows its
  // own activity; now is the recorded clock, for timers. The saved review's
  // edit lines, by agent and net, are newer than the roster's when present.
  let {
    roster,
    lines: edited,
    net,
    loading = false,
    selected,
    now,
    activity,
    onselect,
  }: {
    roster: Roster;
    lines?: Record<string, Counts>;
    net?: Counts | null;
    loading?: boolean;
    selected: string;
    now: number;
    activity: (agent: Agent) => Entry | undefined;
    onselect: (name: string) => void;
  } = $props();

  const agents = $derived(roster.agents);
  const labels = $derived(new Map(agents.map((agent) => [agent.name, agent.label])));
  const known = (c?: Counts | null): c is Counts => !!c && c.Added >= 0 && c.Removed >= 0;
  const changes = (agent: Agent) =>
    edited?.[agent.name] ?? { Added: agent.added ?? 0, Removed: agent.removed ?? 0 };
  const whole = $derived(
    known(net) ? net : { Added: roster.added ?? 0, Removed: roster.removed ?? 0 },
  );
  const lines = (n: number) => `${n} ${n === 1 ? "line" : "lines"}`;
  const unit = (n: number, word: string) => (n === 1 ? word : `${word}s`);
  const plural = (n: number, word: string) => `${n} ${unit(n, word)}`;
  // Count active waits separately from model work, completed work and failures.
  const tallies = $derived(
    (["working", "waiting", "done", "error"] as const)
      .map((state) => ({ state, n: agents.filter((a) => a.state === state).length }))
      .filter(({ n }) => n)
      .map(({ state, n }) => ({
        state,
        n,
        text: state === "error" ? plural(n, "error") : `${n} ${state}`,
      })),
  );
  // A tally that covers every agent says the total itself.
  const single = $derived(tallies.length === 1 && tallies[0].n === agents.length);

  // Session totals; usage that is partly unknown is a lower bound.
  const totals = $derived.by(() => {
    let input = 0,
      output = 0,
      usd = 0,
      tokensKnown = false,
      costKnown = false,
      partial = false;
    for (const a of agents) {
      if (a.inputTokens != null) {
        input += a.inputTokens;
        output += a.outputTokens ?? 0;
        tokensKnown = true;
      }
      if (a.cost != null) {
        usd += a.cost;
        costKnown = true;
      }
      partial ||= !!a.usagePartial || !!a.costPartial;
    }
    return {
      input: tokensKnown ? atLeast(tokens(input), partial) : undefined,
      output: tokensKnown ? atLeast(tokens(output), partial) : undefined,
      cost: costKnown ? atLeast(cost(usd), partial) : undefined,
    };
  });

  // Nested agents show their own name under their parent.
  const name = (agent: Agent) =>
    agent.depth ? agent.label.slice(agent.label.lastIndexOf("/") + 1) : agent.label;

  const dots: Record<Agent["state"], Tone> = {
    working: "running",
    waiting: "hollow",
    done: "done",
    error: "alert",
    idle: "hollow",
    paused: "hollow",
  };
  const tallyInk: Record<string, string> = { working: "text-working-ink", error: "text-negative" };
  const stateInk: Record<string, string> = { working: "text-ink", error: "text-negative" };
  const sub = "pl-4 text-small leading-[18px]";
  const contextPct = (agent: Agent) =>
    agent.context != null && agent.window
      ? Math.min(100, Math.round((agent.context / agent.window) * 100))
      : undefined;

  // An agent that is not working keeps its card to three rows: its values
  // share one line, and the rest show while the card is focused or selected.
  // Hidden values are counted, and the line's title lists them all.
  let clipped = $state<Record<string, number>>({});
  const clip = (agent: Agent) => (node: HTMLElement) => {
    const observer = new ResizeObserver(() => {
      const top = (node.firstElementChild as HTMLElement | null)?.offsetTop ?? 0;
      const n = [...node.children].filter((child) => (child as HTMLElement).offsetTop > top).length;
      if ((clipped[agent.name] ?? 0) !== n) clipped[agent.name] = n;
      node.title = n ? (node.textContent ?? "").replace(/\s+/g, " ").trim() : "";
    });
    observer.observe(node);
    return () => observer.disconnect();
  };
  const unclamped = "group-focus-within/card:max-h-none group-data-[selected=true]/card:max-h-none";
</script>

<!-- A value with its unit in muted ink: 33.72M in, 7 turns. -->
{#snippet metric(value: string, unit: string, title?: string)}
  <span class="whitespace-nowrap" {title}
    ><span class="text-ink">{value}</span> <span class="text-muted">{unit}</span></span
  >
{/snippet}
{#snippet changed(added = 0, removed = 0, title?: string)}
  <span class="whitespace-nowrap" {title}
    ><span class="text-positive">+{added}</span>
    <span class="text-negative">−{removed}</span></span
  >
{/snippet}

<div class="mt-3.5 mb-0.5 flex flex-wrap items-baseline gap-x-2.5 gap-y-1">
  <h2 class="font-semibold">Agents</h2>
  {#if !loading}
    <span class="flex flex-wrap gap-x-2 text-small text-muted tabular-nums">
      {#if single}<span class={["whitespace-nowrap", tallyInk[tallies[0].state]]}
          >{tallies[0].text}</span
        >{:else}{agents.length}{#each tallies as tally (tally.state)}<span
            class="whitespace-nowrap {tallyInk[tally.state] ?? ''}">{tally.text}</span
          >{/each}{/if}
    </span>
  {/if}
</div>
{#if whole.Added || whole.Removed || totals.input || totals.cost}
  <div
    class="mt-2 flex flex-wrap gap-x-3 gap-y-1 rounded-control bg-raised/75 px-2.5 py-1.5 text-small tabular-nums"
  >
    {#if whole.Added || whole.Removed}{@render changed(
        whole.Added,
        whole.Removed,
        `Net lines changed in the workspace (${lines(whole.Added)} added, ${lines(whole.Removed)} removed)`,
      )}{/if}
    {#if totals.input}{@render metric(totals.input, "in", "Input tokens")}{@render metric(
        totals.output!,
        "out",
        "Output tokens",
      )}{/if}
    {#if totals.cost}<span class="text-ink" title="Cost">{totals.cost}</span>{/if}
  </div>
{/if}

{#if loading}
  <div class="mt-3.5 grid gap-3.5" aria-busy="true" aria-label="Loading agents">
    {#each [64, 48] as width, i (i)}<Skeleton
        class="h-[18px] bg-active"
        style="width: {width}%"
      />{/each}
  </div>
{:else if agents.length}
  <ul class="-mx-2.5 mt-2">
    {#each agents as agent (agent.name)}
      {@const time = worked(agent, now)}
      {@const current = agent.state === "working" ? activity(agent) : undefined}
      {@const what = current && doing(current, (name) => stream.label(name))}
      {@const waiting =
        agent.wait && progressText(agent.wait, (name) => labels.get(name) ?? stream.label(name))}
      {@const replied = since(agent.lastResponse, now)}
      {@const edits = changes(agent)}
      {@const color = stream.roleColor(agent.role)}
      {@const compact = agent.state !== "working"}
      <!-- A nested agent hangs from a guide under its parent. -->
      <li
        class={agent.depth ? "border-l border-rule pl-1.5" : undefined}
        style:margin-left={agent.depth ? `${agent.depth * 16 - 6}px` : undefined}
      >
        <div
          data-selected={agent.name === selected ? "true" : undefined}
          class={[
            "group/card relative grid w-full min-w-0 gap-1 rounded-card px-2.5 py-2 text-left transition-colors pointer-coarse:py-[11px]",
            agent.context != null && agent.window && "pb-3 pointer-coarse:pb-[13px]",
            agent.name === selected
              ? "bg-accent-tint"
              : !agent.nonAttachable && "hover:bg-raised/80",
          ]}
        >
          <span class="flex min-w-0 items-center gap-2">
            <button
              type="button"
              class="flex min-w-0 flex-1 items-center gap-2 text-left after:absolute after:inset-0 after:rounded-card focus-visible:outline-none disabled:cursor-default"
              disabled={agent.nonAttachable}
              title={agent.nonAttachable ? "Interactive view unavailable" : undefined}
              aria-current={agent.name === selected ? "true" : undefined}
              onclick={() => onselect(agent.name)}
            >
              <Dot tone={dots[agent.state]} size={8} {color} />
              <span
                class={["min-w-0 truncate font-semibold", color && "text-role"]}
                style:--role={color}>{name(agent)}</span
              >
              {#if agent.role}<Tag {color}>{agent.role}</Tag>{/if}
              {#if time !== undefined}<span
                  class="ml-auto pl-2 font-mono text-small whitespace-nowrap text-muted tabular-nums"
                  title="Time spent working">{duration(time)}</span
                >{/if}
            </button>
          </span>
          <span class="{sub} truncate {stateInk[agent.state] ?? 'text-muted'}" title={waiting}>
            {#if agent.state === "waiting" && agent.wait && waiting}
              <Wait
                text={agent.wait.label || agent.wait.body || ""}
                targets={agent.wait.waits ?? []}
                timeout={agent.wait.waitTimeout}
                reason={agent.wait.waitReason}
                owner={agent.name}
                plain
              />
            {:else if what}
              <span class="sr-only">Working:</span>
              <span class={["font-medium", what.failed ? "text-negative" : "text-working-ink"]}
                >{what.verb}</span
              >
              {#if what.html}<span>{@html what.html}</span>{/if}
            {:else}
              {stateLabel[agent.state]}{#if replied && agent.state !== "working"}<span
                  class="ml-2 text-muted">replied {replied}</span
                >{/if}
            {/if}
          </span>
          {#if preferences.telemetry !== "minimal"}
            <span
              class="{sub} flex items-start gap-3 tabular-nums not-has-[>:first-child>*]:hidden"
            >
              <span
                class={[
                  preferences.telemetry === "compact" && !compact
                    ? "mt-0.5 grid min-w-0 flex-1 grid-cols-2 gap-x-3 gap-y-1"
                    : "flex min-w-0 flex-1 flex-wrap gap-x-3 gap-y-0.5",
                  compact && `max-h-[18px] overflow-hidden ${unclamped}`,
                ]}
                {@attach compact ? clip(agent) : undefined}
              >
                {#if edits.Added || edits.Removed}{@render changed(
                    edits.Added,
                    edits.Removed,
                    "Lines this agent changed",
                  )}{/if}
                {#if agent.inputTokens != null}{@render metric(
                    atLeast(tokens(agent.inputTokens), agent.usagePartial),
                    "in",
                    "Input tokens",
                  )}{@render metric(
                    atLeast(tokens(agent.outputTokens ?? 0), agent.usagePartial),
                    "out",
                    "Output tokens",
                  )}{/if}
                {#if agent.cost != null}<span class="text-ink" title="Cost"
                    >{atLeast(cost(agent.cost), agent.costPartial)}</span
                  >{/if}
                {#if agent.tps != null}<span class="text-muted" title="Output throughput"
                    >{tps(agent.tps)}</span
                  >{/if}
                {#if agent.turns}{@render metric(
                    String(agent.turns),
                    unit(agent.turns, "turn"),
                  )}{/if}
                {#if preferences.telemetry === "detailed"}
                  {#if agent.roundtrips}{@render metric(
                      String(agent.roundtrips),
                      unit(agent.roundtrips, "round trip"),
                    )}{/if}
                  {#if agent.skills?.length}{@render metric(
                      String(agent.skills.length),
                      unit(agent.skills.length, "skill"),
                      agent.skills.join(", "),
                    )}{/if}
                  {#each agent.items ?? [] as item, i (i)}<span class="text-muted">{item}</span
                    >{/each}
                {/if}
                {#if agent.context != null}
                  {@const pct = contextPct(agent)}
                  {@const breakdown = [
                    agent.inputTokens != null && {
                      label: "Input",
                      value: atLeast(tokens(agent.inputTokens), agent.usagePartial),
                    },
                    agent.inputTokens != null && {
                      label: "Output",
                      value: atLeast(tokens(agent.outputTokens ?? 0), agent.usagePartial),
                    },
                    agent.cost != null && {
                      label: "Cost",
                      value: atLeast(cost(agent.cost), agent.costPartial),
                    },
                    agent.tps != null && {
                      label: "Throughput",
                      value: tps(agent.tps),
                    },
                    agent.turns && { label: "Turns", value: String(agent.turns) },
                  ].filter((row): row is { label: string; value: string } => Boolean(row))}
                  <ContextCard
                    percent={pct}
                    usedLabel={tokens(agent.context)}
                    limitLabel={agent.window ? tokens(agent.window) : undefined}
                    rows={breakdown}
                    label={`Context breakdown for ${name(agent)}`}
                    class={preferences.telemetry === "compact" && !compact
                      ? "relative z-10 col-span-2"
                      : "relative z-10"}
                  >
                    <span class="whitespace-nowrap" title="Context used"
                      >{#if pct !== undefined}<span class="text-ink">{pct}%</span>
                        <span class="text-muted">·</span>{/if}
                      <span class="text-ink">{tokens(agent.context)}</span>
                      <span class="text-muted"
                        >{agent.window ? `of ${tokens(agent.window)} context` : "context"}</span
                      ></span
                    >
                  </ContextCard>
                {/if}
              </span>
              <!-- Screen readers already read every value. -->
              {#if compact && clipped[agent.name]}<span
                  class="flex-none text-muted group-focus-within/card:hidden group-data-[selected=true]/card:hidden"
                  aria-hidden="true">+{clipped[agent.name]}</span
                >{/if}
            </span>
          {/if}
          <!-- The context bar runs along the card's foot rather than taking a row. -->
          {#if agent.context != null && agent.window}
            {@const pct = contextPct(agent) ?? 0}
            <Progress
              class={[
                "pointer-events-none absolute inset-x-2.5 bottom-1 h-[3px] w-auto bg-active",
                pct >= 90
                  ? "[&_[data-slot=progress-indicator]]:bg-negative"
                  : pct >= 70
                    ? "[&_[data-slot=progress-indicator]]:bg-working"
                    : "[&_[data-slot=progress-indicator]]:bg-accent",
              ]}
              value={Math.max(0, Math.min(agent.context, agent.window))}
              max={agent.window}
              aria-hidden="true"
            />
          {/if}
        </div>
      </li>
    {/each}
  </ul>
{:else}
  <p class="{emptyBox} mt-2.5">No agents have started yet.</p>
{/if}
