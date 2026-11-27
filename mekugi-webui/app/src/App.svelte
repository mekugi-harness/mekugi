<script lang="ts">
  import { tick } from "svelte";
  import type { InitialInput } from "./lib/attachments";
  import { hub, type Machine } from "./lib/hub.svelte";
  import MachineForm from "./lib/MachineForm.svelte";
  import Mark from "./lib/Mark.svelte";
  import MachineSettings from "./lib/MachineSettings.svelte";
  import NewThread from "./lib/NewThread.svelte";
  import QuickSwitcher from "./lib/QuickSwitcher.svelte";
  import Sidebar from "./lib/Sidebar.svelte";
  import ThreadView from "./lib/ThreadView.svelte";
  import Button from "./lib/ui/Button.svelte";
  import Notice from "./lib/ui/Notice.svelte";
  import * as Tooltip from "./lib/ui/tooltip";
  import { view } from "./lib/view.svelte";

  // The sidebar sits beside the main area where it fits and is a drawer over
  // it otherwise.
  const media = matchMedia("(min-width: 900px)");
  let wide = $state(media.matches);
  let sidebar = $state(media.matches);
  $effect(() => {
    const change = () => (wide = sidebar = media.matches);
    media.addEventListener("change", change);
    return () => media.removeEventListener("change", change);
  });

  let settings = $state<{ focus?: Machine }>();
  let switcher = $state(false);
  let firstInput = $state.raw<{ machine: string; input: InitialInput }>();

  // A link to a thread on a machine this page has not connected opens a new
  // thread on a connected machine instead.
  const machine = $derived(hub.selected ?? hub.machines[0]);
  const thread = $derived(hub.selected && view.session ? view.session : undefined);
  const hide = () => {
    if (!wide) sidebar = false;
  };
  // Below the breakpoint the sidebar is a modal drawer: it takes focus, the
  // sheet behind it is inert, and Escape or the scrim closes it, returning
  // focus to the button that opened it.
  const modal = $derived(sidebar && !wide);
  let drawer = $state<HTMLElement>();
  $effect(() => {
    if (modal) drawer?.querySelector<HTMLElement>("[aria-current=page], a, button")?.focus();
  });
  function closeDrawer() {
    sidebar = false;
    void tick().then(() =>
      document.querySelector<HTMLElement>('button[aria-label="Show sidebar"]')?.focus(),
    );
  }
  function onkeydown(event: KeyboardEvent) {
    if (modal && event.key === "Escape" && !event.defaultPrevented) {
      event.preventDefault();
      closeDrawer();
      return;
    }
    if ((event.metaKey || event.ctrlKey) && !event.altKey && !event.shiftKey) {
      if (event.key.toLowerCase() === "k" && !settings) {
        event.preventDefault();
        switcher = !switcher;
        return;
      }
      if (event.key === "\\" && !settings && !switcher) {
        event.preventDefault();
        if (modal) closeDrawer();
        else sidebar = !sidebar;
      }
    }
  }
</script>

<svelte:window {onkeydown} />

<Tooltip.Provider delayDuration={300}>
  <div class="relative flex h-dvh bg-surface">
    {#if sidebar}
      <div
        bind:this={drawer}
        class={wide
          ? "min-h-0 w-sidebar flex-none border-r border-rule/70 bg-ground"
          : "fixed inset-y-0 left-0 z-30 w-[min(var(--container-sidebar),86vw)] border-r border-rule bg-ground shadow-float"}
      >
        <Sidebar
          onhide={() => (modal ? closeDrawer() : (sidebar = false))}
          onmachines={(focus) => {
            hide();
            settings = { focus };
          }}
          onswitcher={() => {
            hide();
            switcher = true;
          }}
          onnavigate={hide}
        />
      </div>
      {#if !wide}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions (the sidebar's own button also hides it) -->
        <div class="fixed inset-0 z-29 bg-scrim backdrop-blur-[2px]" onclick={closeDrawer}></div>
      {/if}
    {/if}

    <!-- Seamless edge-to-edge main canvas beside the sidebar. -->
    <div class="relative flex min-w-0 flex-1 flex-col overflow-hidden bg-surface" inert={modal}>
      {#if hub.storageError && !settings}<Notice role="alert" class="m-3 flex-none"
          >{hub.storageError}</Notice
        >{/if}
      {#if machine && thread}
        {#key machine.id}
          <ThreadView
            {machine}
            id={thread}
            {sidebar}
            onsidebar={() => (sidebar = true)}
            initial={firstInput?.machine === machine.id &&
            firstInput.input.started.thread.id === thread
              ? firstInput.input
              : undefined}
            oninitial={() => (firstInput = undefined)}
          />
        {/key}
      {:else}
        <header class="flex h-bar flex-none items-center gap-1 px-2 pt-safe">
          {#if !sidebar}<Button
              variant="quiet"
              icon="sidebar"
              label="Show sidebar"
              tooltip
              onclick={() => (sidebar = true)}
            />{/if}
        </header>
        {#if machine}
          {#if view.session && !hub.selected}<Notice
              class="mx-auto w-[calc(100%-2rem)] max-w-column"
            >
              That thread's machine is not connected in this page. Add it under Machines to open the
              thread.
            </Notice>{/if}
          <NewThread
            {machine}
            onopened={(machine, input) => {
              firstInput = { machine, input };
            }}
          />
        {:else}
          <section
            class="grid min-h-0 flex-1 items-start justify-items-center overflow-y-auto px-4 pt-[11dvh] pb-8"
            aria-labelledby="welcome-heading"
          >
            <div class="grid w-[min(100%,440px)] gap-6">
              <header class="grid gap-2.5">
                <div class="mb-1 flex items-center gap-2.5">
                  <Mark size={28} />
                  <span class="text-small font-medium tracking-wide text-muted uppercase"
                    >Mekugi</span
                  >
                </div>
                <h1 id="welcome-heading" class="text-heading font-semibold tracking-[-0.02em]">
                  Connect a machine
                </h1>
                <p class="leading-[22px] text-muted">
                  Threads run on your execution machines. Connect one through its connector to start
                  a thread or follow work already running there.
                </p>
              </header>
              <MachineForm />
            </div>
          </section>
        {/if}
      {/if}
    </div>
  </div>
</Tooltip.Provider>

{#if switcher}
  <QuickSwitcher
    onclose={() => (switcher = false)}
    onmachines={() => (settings = {})}
    ontogglesidebar={() => (sidebar = !sidebar)}
  />
{/if}
{#if settings}
  <MachineSettings focus={settings.focus} onclose={() => (settings = undefined)} />
{/if}
