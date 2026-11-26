<script lang="ts">
  import type { Snippet } from "svelte";
  import * as Popover from "../popover";
  import { Progress } from "../progress";
  import { Separator } from "../separator";

  let {
    percent,
    usedLabel,
    limitLabel,
    rows = [],
    label = "Context window breakdown",
    class: className = "",
    children,
  }: {
    percent?: number;
    usedLabel: string;
    limitLabel?: string;
    rows?: { label: string; value: string }[];
    label?: string;
    class?: string;
    children: Snippet;
  } = $props();
</script>

<Popover.Root>
  <Popover.Trigger>
    {#snippet child({ props })}
      <button
        type="button"
        {...props}
        aria-label={label}
        class={[
          "inline-flex min-w-0 items-center rounded-sm text-left transition-colors hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent",
          className,
        ]}
      >
        {@render children()}
      </button>
    {/snippet}
  </Popover.Trigger>
  <Popover.Content
    data-ai-element="context"
    align="end"
    sideOffset={6}
    class="flex w-60 flex-col gap-2.5 rounded-float border-0 bg-surface p-3 text-small shadow-float ring-1 ring-border/70"
  >
    <div class="flex items-center justify-between gap-2">
      <span class="font-medium text-ink">Context window</span>
      <span class="font-mono text-tiny text-muted tabular-nums">
        {usedLabel}{#if limitLabel}
          / {limitLabel}{/if}
      </span>
    </div>
    {#if percent !== undefined}
      <Progress
        value={Math.max(0, Math.min(100, percent))}
        max={100}
        aria-label="Context used"
        class={[
          "h-1.5 bg-active",
          percent >= 90
            ? "[&_[data-slot=progress-indicator]]:bg-negative"
            : percent >= 70
              ? "[&_[data-slot=progress-indicator]]:bg-working"
              : "[&_[data-slot=progress-indicator]]:bg-accent",
        ]}
      />
    {/if}
    {#if rows.length}
      <Separator class="bg-rule" />
      <div class="grid gap-1">
        {#each rows as row (row.label)}
          <div class="flex items-center justify-between gap-3">
            <span class="text-muted">{row.label}</span>
            <span class="font-mono text-ink tabular-nums">{row.value}</span>
          </div>
        {/each}
      </div>
    {/if}
  </Popover.Content>
</Popover.Root>
