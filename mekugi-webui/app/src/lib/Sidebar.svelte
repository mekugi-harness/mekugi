<script lang="ts">
  import Path from "./Path.svelte";
  import { basename, statusText } from "./format";
  import { hub, type Machine } from "./hub.svelte";
  import Mark from "./Mark.svelte";
  import type { Listed } from "./protocol";
  import { selectionHash } from "./transport";
  import Button from "./ui/Button.svelte";
  import Dot from "./ui/Dot.svelte";
  import Icon from "./ui/Icon.svelte";
  import { Skeleton } from "./ui/skeleton";
  import Tag from "./ui/Tag.svelte";
  import { appearance, themes, type Theme } from "./theme.svelte";
  import { select, view } from "./view.svelte";

  // Every connected machine's threads, grouped under the machine. The footer
  // opens machine settings and chooses the page's theme; a machine whose
  // token was refused offers its replacement in place.
  let {
    onhide,
    onmachines,
    onswitcher,
    onnavigate,
  }: {
    onhide: () => void;
    onmachines: (machine?: Machine) => void;
    onswitcher?: () => void;
    onnavigate: () => void;
  } = $props();
  let collapsed = $state<Record<string, boolean>>({});
  const name = (s: Listed) =>
    s.title || (s.workspace ? basename(s.workspace) : undefined) || s.thread || "New thread";
  function start(machine?: Machine) {
    select(
      machine?.id ??
        view.machine ??
        hub.machines.find((m) => m.connection === "open")?.id ??
        hub.machines[0]?.id,
    );
    onnavigate();
    queueMicrotask(() => document.getElementById("new-text")?.focus());
  }
  // The theme button shows the current choice and moves to the next.
  const themeIcons = { system: "monitor", light: "sun", dark: "moon" } as const;
  const title = (theme: Theme) => theme[0].toUpperCase() + theme.slice(1);
  // Flat transparent action rows; only active thread links carry a subtle pill highlight.
  const flatRow =
    "flex w-full items-center rounded-lg text-left text-ink/90 transition-colors hover:bg-active/45 hover:text-ink max-sm:min-h-11 pointer-coarse:min-h-11";
  const threadRow =
    "flex w-full items-center rounded-full text-left text-ink/90 transition-colors hover:bg-active/45 hover:text-ink aria-[current=page]:bg-active/75 aria-[current=page]:text-ink max-sm:min-h-11 pointer-coarse:min-h-11";
</script>

<nav class="flex h-full min-h-0 flex-col px-3 pt-safe pb-safe" aria-label="Threads">
  <div class="flex h-13 items-center justify-between pl-2">
    <span class="flex items-center gap-2.5 text-[15px] font-semibold tracking-[-0.015em] text-ink"
      ><Mark />Mekugi</span
    >
    <div class="flex items-center gap-0.5">
      {#if onswitcher}
        <Button
          variant="quiet"
          icon="search"
          label="Quick switcher"
          title="Quick switcher (⌘K)"
          tooltip
          onclick={onswitcher}
        />
      {/if}
      <Button
        variant="quiet"
        icon="sidebar"
        label="Hide sidebar"
        title="Hide sidebar (⌘\)"
        tooltip
        onclick={onhide}
      />
    </div>
  </div>
  <button
    type="button"
    class="{flatRow} min-h-9 gap-3 px-2.5 font-medium"
    aria-current={!view.session && !!view.machine ? "page" : undefined}
    onclick={() => start()}
  >
    <span class="flex flex-none text-ink/75"><Icon name="compose" size={16} /></span>
    <span class="flex-1 truncate">New thread</span>
  </button>

  <div class="mt-4 min-h-0 flex-1 overflow-y-auto overscroll-contain">
    {#each hub.machines as machine (machine.id)}
      {@const state = machine.state}
      {@const open = !collapsed[machine.id]}
      <section class="group/machine not-first:mt-5" aria-labelledby="machine-{machine.id}">
        <div class="relative flex items-center">
          <button
            type="button"
            id="machine-{machine.id}"
            class="flex min-h-8 min-w-0 flex-1 items-center gap-2 rounded-full px-3 text-left text-small font-medium text-muted transition-colors hover:bg-active/50 hover:text-ink max-sm:min-h-11 pointer-coarse:min-h-11"
            aria-label="{machine.label}, {state.text}"
            aria-expanded={open}
            onclick={() => (collapsed[machine.id] = open)}
          >
            <span class={["inline-flex transition-transform duration-150", !open && "-rotate-90"]}
              ><Icon name="chevron" size={13} /></span
            >
            <Dot tone={state.tone || "idle"} size={6} />
            <span class="min-w-0 flex-1 truncate">{machine.label}</span>
            {#if machine.sessions?.length}
              <span
                class="pr-0.5 text-tiny font-medium text-muted tabular-nums transition-opacity group-hover/machine:opacity-0 group-focus-within/machine:opacity-0 touch:hidden"
                aria-hidden="true">{machine.sessions.length}</span
              >
            {/if}
          </button>
          <Button
            variant="quiet"
            icon="plus"
            size="sm"
            label="New thread on {machine.label}"
            class="absolute right-0.5 opacity-0 transition-opacity group-hover/machine:opacity-100 group-focus-within/machine:opacity-100 focus-visible:opacity-100 touch:static touch:opacity-100"
            onclick={() => start(machine)}
          />
        </div>
        {#if state.tone === "alert" || state.tone === "warn"}
          <p
            class={[
              "mt-0.5 px-3 py-1 text-small",
              state.tone === "alert" ? "text-negative" : "text-warning-ink",
            ]}
          >
            {state.text}{#if machine.connection === "required"}
              <button type="button" class="ml-2 underline" onclick={() => onmachines(machine)}
                >Replace token</button
              >{/if}
          </p>
        {/if}
        {#if open}
          {#if !machine.sessions}
            {#if machine.connection === "connecting"}<Skeleton
                class="mx-3 mt-2 h-5 bg-active"
                aria-label="Loading threads"
              />{/if}
          {:else if !machine.sessions.length}
            <p class="mt-0.5 px-3 py-1.5 text-small text-muted">No open threads</p>
          {:else}
            <ul class="mt-1 space-y-0.5">
              {#each machine.sessions as s (s.id)}
                {@const status = statusText(s.status)}
                {@const current = view.machine === machine.id && view.session === s.id}
                <li>
                  <a
                    class="{threadRow} min-h-9 gap-2.5 px-3 py-1.5 no-underline aria-[current=page]:font-medium"
                    href={selectionHash({ machine: machine.id, session: s.id })}
                    aria-current={current ? "page" : undefined}
                    title={[name(s), s.workspace, status].filter(Boolean).join("\n")}
                    onclick={onnavigate}
                  >
                    <span class="flex min-w-0 flex-1 flex-col gap-0.5">
                      <span class="truncate leading-5"
                        >{#if !s.title && s.workspace}<Path
                            path={name(s)}
                            title={s.workspace}
                          />{:else}{name(s)}{/if}</span
                      >
                      {#if s.title && s.workspace}<span
                          class="truncate text-tiny font-normal text-muted"
                          ><Path path={basename(s.workspace)} title={s.workspace} /></span
                        >{/if}
                    </span>
                    {#if s.waiting}<span
                        class="min-w-[18px] flex-none rounded-full bg-warning-tint px-[5px] text-center text-tiny font-medium text-warning-ink tabular-nums ring-1 ring-warning-edge ring-inset"
                        title="Prompts waiting">{s.waiting}</span
                      >{/if}
                    <Dot tone={s.status.alert ? "alert" : s.status.running ? "running" : "none"} />
                    {#if status || s.waiting}<span class="sr-only"
                        >{status}{s.waiting ? `, ${s.waiting} waiting` : ""}</span
                      >{/if}
                  </a>
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
      </section>
    {:else}
      <p class="mt-0.5 px-3 py-1.5 text-small text-muted">
        Connect a machine to see its threads here.
      </p>
    {/each}
  </div>

  <div class="pt-2 pb-3">
    <div class="flex items-center gap-1">
      <button
        type="button"
        class="{flatRow} min-h-9 min-w-0 flex-1 gap-3 px-2.5 font-medium"
        onclick={() => onmachines()}
      >
        <span class="flex flex-none text-ink/75"><Icon name="server" size={16} /></span>Machines
        {#if hub.machines.length}<span
            class="ml-auto rounded-full bg-active px-1.5 py-px text-tiny font-medium text-muted tabular-nums"
            aria-hidden="true">{hub.machines.length}</span
          >{/if}
      </button>
      <Button
        variant="quiet"
        icon={themeIcons[appearance.choice]}
        label="Theme: {title(appearance.choice)}"
        tooltip
        onclick={() =>
          appearance.set(themes[(themes.indexOf(appearance.choice) + 1) % themes.length])}
      />
    </div>
    {#if appearance.storageError}<p role="status" class="mt-1 px-2.5 text-small text-muted">
        {appearance.storageError}
      </p>{/if}
  </div>
</nav>
