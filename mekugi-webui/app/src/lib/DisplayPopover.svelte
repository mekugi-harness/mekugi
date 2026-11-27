<script lang="ts">
  import {
    preferences,
    type HookMode,
    type Palette,
    type TelemetryMode,
    type ThoughtMode,
    type TimestampMode,
    type ViewPreset,
  } from "./preferences.svelte";
  import { semantic } from "./semantic.svelte";
  import Icon from "./ui/Icon.svelte";
  import * as Kbd from "./ui/kbd";
  import * as PopoverPrimitive from "./ui/popover";
  import { Separator } from "./ui/separator";
  import * as ToggleGroup from "./ui/toggle-group";

  let {
    hiddenHooks = 0,
    predictionsAvailable,
  }: {
    hiddenHooks?: number;
    predictionsAvailable: boolean;
  } = $props();

  let open = $state(false);
  let mounted = $state(false);

  const presetOptions: { id: ViewPreset; label: string }[] = [
    { id: "focus", label: "Focus" },
    { id: "standard", label: "Standard" },
    { id: "debug", label: "Debug" },
  ];
  const presetNames: Record<ViewPreset | "custom", string> = {
    focus: "Focus",
    standard: "Standard",
    debug: "Debug",
    custom: "Custom",
  };
  const hookOptions: { id: HookMode; label: string }[] = [
    { id: "off", label: "Off" },
    { id: "errors", label: "Errors" },
    { id: "all", label: "All" },
  ];
  const thoughtOptions: { id: ThoughtMode; label: string }[] = [
    { id: "hidden", label: "Hidden" },
    { id: "collapsed", label: "Collapsed" },
    { id: "expanded", label: "Open" },
  ];
  const timestampOptions: { id: TimestampMode; label: string }[] = [
    { id: "hover", label: "Hover" },
    { id: "auto", label: "Always" },
    { id: "off", label: "Off" },
  ];
  const telemetryOptions: { id: TelemetryMode; label: string }[] = [
    { id: "minimal", label: "Minimal" },
    { id: "compact", label: "Compact" },
    { id: "detailed", label: "Full" },
  ];
  const paletteCards: {
    id: Palette;
    label: string;
    swatch: string;
    dotA: string;
    dotB: string;
  }[] = [
    {
      id: "sumi",
      label: "Sumi",
      swatch: "bg-swatch-sumi",
      dotA: "bg-live",
      dotB: "bg-working",
    },
    {
      id: "violet",
      label: "Violet",
      swatch: "bg-swatch-violet",
      dotA: "bg-accent",
      dotB: "bg-working",
    },
    {
      id: "mono",
      label: "Mono",
      swatch: "bg-swatch-mono",
      dotA: "bg-on-primary/70",
      dotB: "bg-working",
    },
  ];

  const triggerLabel = $derived(
    hiddenHooks
      ? "Display settings (" + hiddenHooks + (hiddenHooks === 1 ? " hook" : " hooks") + " hidden)"
      : "Display settings",
  );
  const pillText = $derived(
    hiddenHooks
      ? presetNames[preferences.preset] +
          " · " +
          hiddenHooks +
          (hiddenHooks === 1 ? " hook" : " hooks") +
          " hidden"
      : presetNames[preferences.preset],
  );
  const controlRow =
    "flex items-center justify-between gap-3 max-sm:flex-col max-sm:items-stretch max-sm:gap-1.5";
  const groupTrack = "grid grid-cols-3 gap-0.5 rounded-control bg-raised p-0.5";
  const groupItem =
    "h-7 min-w-0 flex-1 rounded-[5px] border-0 bg-transparent px-2 text-small font-medium text-muted shadow-none transition-colors hover:bg-transparent hover:text-ink data-[state=on]:bg-surface data-[state=on]:text-ink data-[state=on]:shadow-xs dark:data-[state=on]:bg-active max-sm:h-11";
</script>

<PopoverPrimitive.Root
  bind:open
  onOpenChange={(next) => {
    if (next) mounted = true;
  }}
>
  <PopoverPrimitive.Trigger>
    {#snippet child({ props })}
      <button
        type="button"
        {...props}
        aria-label={triggerLabel}
        title={triggerLabel}
        class="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-control px-2 text-small font-medium text-muted transition-colors hover:bg-raised hover:text-ink aria-expanded:bg-active aria-expanded:text-ink max-lg:size-8 max-lg:justify-center max-lg:px-0 max-sm:min-h-11 max-sm:min-w-11"
      >
        <span class="flex flex-none"><Icon name="sliders" size={14} /></span>
        <span class="whitespace-nowrap text-ink tabular-nums max-lg:hidden">{pillText}</span>
        <span class="flex flex-none max-lg:hidden"><Icon name="chevron" size={12} /></span>
      </button>
    {/snippet}
  </PopoverPrimitive.Trigger>
  {#if mounted}
    <PopoverPrimitive.Content
      forceMount
      align="end"
      sideOffset={6}
      collisionPadding={12}
      aria-label={triggerLabel}
      style={open ? undefined : "display: none"}
      class="max-h-[min(calc(100dvh-var(--topbar)-24px),var(--bits-popover-content-available-height))] w-[min(372px,calc(100vw-24px))] overflow-y-auto rounded-float border border-border bg-surface p-4 text-small shadow-float ring-0"
    >
      <div class="grid gap-3.5">
        <ToggleGroup.Root
          type="single"
          bind:value={
            () => (preferences.preset === "custom" ? "" : preferences.preset),
            (value) => {
              if (value) preferences.applyPreset(value as ViewPreset);
            }
          }
          class={groupTrack}
          aria-label="View preset"
        >
          {#each presetOptions as option (option.id)}
            <ToggleGroup.Item value={option.id} class={groupItem}>{option.label}</ToggleGroup.Item>
          {/each}
        </ToggleGroup.Root>

        <Separator class="bg-rule" />

        <div class="grid gap-2.5">
          {#if predictionsAvailable}
            <div class={controlRow}>
              <span class="font-medium text-ink">Predictions</span>
              <ToggleGroup.Root
                type="single"
                bind:value={
                  () => (semantic.predictionsEnabled ? "on" : "off"),
                  (value) => {
                    if (value) semantic.setPredictions(value === "on");
                  }
                }
                class="grid w-[238px] shrink-0 grid-cols-2 gap-0.5 rounded-control bg-raised p-0.5 max-sm:w-full"
                aria-label="Predictions"
              >
                <ToggleGroup.Item value="off" class={groupItem}>Off</ToggleGroup.Item>
                <ToggleGroup.Item value="on" class={groupItem}>On</ToggleGroup.Item>
              </ToggleGroup.Root>
            </div>
          {/if}
          <div class={controlRow}>
            <span class="font-medium text-ink">Hooks</span>
            <ToggleGroup.Root
              type="single"
              bind:value={
                () => preferences.hooks,
                (value) => {
                  if (value) preferences.setHooks(value as HookMode);
                }
              }
              class="{groupTrack} w-[238px] shrink-0 max-sm:w-full"
              aria-label="Hooks visibility"
            >
              {#each hookOptions as option (option.id)}
                <ToggleGroup.Item value={option.id} class={groupItem}
                  >{option.label}</ToggleGroup.Item
                >
              {/each}
            </ToggleGroup.Root>
          </div>

          <div class={controlRow}>
            <span class="font-medium text-ink">Thoughts</span>
            <ToggleGroup.Root
              type="single"
              bind:value={
                () => preferences.thoughts,
                (value) => {
                  if (value) preferences.setThoughts(value as ThoughtMode);
                }
              }
              class="{groupTrack} w-[238px] shrink-0 max-sm:w-full"
              aria-label="Thoughts"
            >
              {#each thoughtOptions as option (option.id)}
                <ToggleGroup.Item value={option.id} class={groupItem}
                  >{option.label}</ToggleGroup.Item
                >
              {/each}
            </ToggleGroup.Root>
          </div>

          <div class={controlRow}>
            <span class="font-medium text-ink">Timestamps</span>
            <ToggleGroup.Root
              type="single"
              bind:value={
                () => preferences.timestamps,
                (value) => {
                  if (value) preferences.setTimestamps(value as TimestampMode);
                }
              }
              class="{groupTrack} w-[238px] shrink-0 max-sm:w-full"
              aria-label="Inline timestamps"
            >
              {#each timestampOptions as option (option.id)}
                <ToggleGroup.Item value={option.id} class={groupItem}
                  >{option.label}</ToggleGroup.Item
                >
              {/each}
            </ToggleGroup.Root>
          </div>

          <div class={controlRow}>
            <span class="font-medium text-ink">Telemetry</span>
            <ToggleGroup.Root
              type="single"
              bind:value={
                () => preferences.telemetry,
                (value) => {
                  if (value) preferences.setTelemetry(value as TelemetryMode);
                }
              }
              class="{groupTrack} w-[238px] shrink-0 max-sm:w-full"
              aria-label="Agent telemetry"
            >
              {#each telemetryOptions as option (option.id)}
                <ToggleGroup.Item value={option.id} class={groupItem}
                  >{option.label}</ToggleGroup.Item
                >
              {/each}
            </ToggleGroup.Root>
          </div>
        </div>

        <Separator class="bg-rule" />

        <div class="grid gap-2">
          <span class="text-small font-medium text-muted">Chroma &amp; Theme</span>
          <div class="grid grid-cols-3 gap-2" role="group" aria-label="Chroma palette">
            {#each paletteCards as card (card.id)}
              {@const selected = preferences.palette === card.id}
              <button
                type="button"
                aria-pressed={selected}
                class={[
                  "grid overflow-hidden rounded-card bg-raised text-left transition-all max-sm:min-h-11",
                  selected ? "ring-2 ring-accent" : "hover:bg-active",
                ]}
                onclick={() => preferences.setPalette(card.id)}
              >
                <span
                  class={["flex items-center gap-1.5 px-2 py-2", card.swatch]}
                  aria-hidden="true"
                >
                  <span class={["size-2 rounded-full", card.dotA]}></span>
                  <span class={["size-2 rounded-full", card.dotB]}></span>
                  {#if selected}<span class="ml-auto text-tiny text-on-primary">✓</span>{/if}
                </span>
                <span class="px-2 py-1.5 text-small font-medium text-ink">{card.label}</span>
              </button>
            {/each}
          </div>
        </div>

        {#if preferences.storageError}
          <p role="status" class="text-small text-muted">{preferences.storageError}</p>
        {/if}

        <Separator class="bg-rule" />

        <div class="flex items-center justify-between text-small text-muted">
          <span class="inline-flex items-center gap-1.5"
            ><Kbd.Root class="bg-raised font-mono text-tiny text-muted">⌘⇧H</Kbd.Root> hooks</span
          >
          <span class="inline-flex items-center gap-1.5"
            ><Kbd.Root class="bg-raised font-mono text-tiny text-muted">⌘⇧D</Kbd.Root> debug</span
          >
        </div>
      </div>
    </PopoverPrimitive.Content>
  {/if}
</PopoverPrimitive.Root>
