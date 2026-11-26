<script lang="ts">
  import Alert from "./ui/Alert.svelte";
  import { tick, untrack } from "svelte";
  import { hub, type Machine } from "./hub.svelte";
  import MachineForm from "./MachineForm.svelte";
  import Notice from "./ui/Notice.svelte";
  import Button from "./ui/Button.svelte";
  import Dialog from "./ui/Dialog.svelte";
  import Dot from "./ui/Dot.svelte";
  import Field from "./ui/Field.svelte";
  import { input } from "./ui/styles";

  // Every machine this page is connected to: add one, replace a refused
  // token, or remove one. Removing leaves the machine's threads running.
  let { focus, onclose }: { focus?: Machine; onclose: () => void } = $props();
  type Editing =
    | { kind: "add" | "fleet" | "sandbox" }
    | { kind: "replace" | "remove"; id: string }
    | undefined;
  // Opening for a machine starts on its token; an empty page starts on adding one.
  let editing = $state<Editing>(
    untrack(() =>
      focus ? { kind: "replace", id: focus.id } : hub.machines.length ? undefined : { kind: "add" },
    ),
  );
  let token = $state("");
  let fleetJson = $state("");
  let fleetStatus = $state<string>();
  let error = $state<string>();
  const sandboxCommand = "sbx run --config sandbox-kit/mekugi-dev.yaml";
  let copiedSandbox = $state(false);
  function edit(next: Editing) {
    editing = next;
    token = "";
    fleetJson = "";
    fleetStatus = undefined;
    error = undefined;
    copiedSandbox = false;
    tick().then(() => document.querySelector<HTMLElement>("[role=dialog] [data-first]")?.focus());
  }
  async function importFleet(event: SubmitEvent) {
    event.preventDefault();
    error = undefined;
    fleetStatus = undefined;
    const res = await hub.importFleetJson(fleetJson);
    if (res.error) {
      error = res.error;
      return;
    }
    fleetStatus = `Imported ${res.added} devbox${res.added === 1 ? "" : "es"}${res.skipped ? ` (${res.skipped} skipped)` : ""}.`;
    if (res.added > 0) edit(undefined);
  }
  function reconnect(event: SubmitEvent, machine: Machine) {
    event.preventDefault();
    error = hub.authorize(machine.id, token);
    if (!error) edit(undefined);
  }
  // What sits under a machine's row, in line with its name.
  const inset = "mt-2 ml-[19px]";
  const well = `${inset} grid gap-2.5 rounded-card bg-raised p-3`;
</script>

<Dialog title="Machines" {onclose}>
  {#if hub.storageError}<Notice role="alert" class="mb-3">{hub.storageError}</Notice>{/if}
  {#if hub.machines.length}
    <ul class="mb-4 grid">
      {#each hub.machines as machine (machine.id)}
        {@const state = machine.state}
        <li class="border-t border-rule py-3 first:border-t-0 first:pt-0">
          <div class="flex items-center gap-3 max-sm:flex-wrap">
            <Dot tone={state.tone || "idle"} />
            <div class="grid min-w-0 flex-1">
              <strong class="font-semibold">{machine.label}</strong>
              <span class="truncate font-mono text-small text-muted">{machine.endpoint}</span>
            </div>
            <span
              class={[
                "flex-none text-small",
                state.tone === "alert" ? "text-negative" : "text-muted",
              ]}
              >{state.text}{#if machine.latencyMs !== undefined && state.tone === "open"}
                · {machine.latencyMs}ms{/if}</span
            >
          </div>
          {#if machine.error && state.tone !== "open"}<p class="{inset} text-small text-negative">
              {machine.error}
            </p>{/if}
          {#if editing?.kind === "replace" && editing.id === machine.id}
            <form class={well} onsubmit={(event) => reconnect(event, machine)}>
              <Field label="Access token"
                ><input
                  class="{input} font-mono"
                  data-first
                  required
                  type="password"
                  bind:value={token}
                  autocomplete="off"
                  spellcheck="false"
                /></Field
              >
              {#if error}<Alert>{error}</Alert>{/if}
              <div class="flex justify-end gap-2">
                <Button variant="quiet" onclick={() => edit(undefined)}>Cancel</Button>
                <Button type="submit" variant="primary">Reconnect</Button>
              </div>
            </form>
          {:else if editing?.kind === "remove" && editing.id === machine.id}
            <div class={well} role="group" aria-label="Remove {machine.label}">
              <p>
                Remove {machine.label} and its saved token from this browser? Its threads keep running
                on the machine, and you can add it again with its token.
              </p>
              <div class="flex justify-end gap-2">
                <Button variant="quiet" data-first onclick={() => edit(undefined)}>Keep</Button>
                <Button
                  variant="danger"
                  onclick={() => {
                    hub.remove(machine.id);
                    edit(undefined);
                  }}>Remove</Button
                >
              </div>
            </div>
          {:else}
            <div class="{inset} flex gap-2">
              <Button
                aria-label="Replace token for {machine.label}"
                onclick={() => edit({ kind: "replace", id: machine.id })}>Replace token</Button
              >
              <Button
                variant="quiet-danger"
                aria-label="Remove {machine.label}"
                onclick={() => edit({ kind: "remove", id: machine.id })}>Remove</Button
              >
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
  {#if editing?.kind === "add"}
    <section class="pt-1" aria-label="Add machine">
      {#if hub.machines.length}<h3 class="mb-3 font-semibold">Add machine</h3>{/if}
      <MachineForm
        onadded={onclose}
        oncancel={hub.machines.length ? () => edit(undefined) : undefined}
      />
    </section>
  {:else if editing?.kind === "fleet"}
    <form class="grid gap-2.5 pt-1" onsubmit={importFleet}>
      <h3 class="font-semibold">Import DEVBOXES_JSON fleet</h3>
      <Field label="DEVBOXES_JSON">
        <textarea
          class="{input} min-h-24 font-mono text-small"
          data-first
          required
          bind:value={fleetJson}
          placeholder={'[{"name":"netcup","endpoint":"https://box.example","token":"..."}]'}
        ></textarea>
      </Field>
      {#if error}<Alert>{error}</Alert>{/if}
      {#if fleetStatus}<Notice>{fleetStatus}</Notice>{/if}
      <div class="flex justify-end gap-2">
        <Button variant="quiet" onclick={() => edit(undefined)}>Cancel</Button>
        <Button type="submit" variant="primary">Import fleet</Button>
      </div>
    </form>
  {:else if editing?.kind === "sandbox"}
    <div class="grid gap-2.5 pt-1">
      <h3 class="font-semibold">Launch ephemeral mekugi-dev sandbox</h3>
      <p class="text-small text-muted">
        Spawns an isolated Docker Sandbox using <code>sandbox-kit/mekugi-dev.yaml</code> pre-wired
        with <code>mekugi-app-server</code> so you can attach it to this workbench fleet.
      </p>
      <pre
        class="overflow-x-auto rounded-card bg-raised p-2.5 font-mono text-small">{sandboxCommand}</pre>
      <div class="flex justify-end gap-2">
        <Button variant="quiet" onclick={() => edit(undefined)}>Close</Button>
        <Button
          variant="primary"
          onclick={async () => {
            await navigator.clipboard?.writeText(sandboxCommand);
            copiedSandbox = true;
          }}>{copiedSandbox ? "Copied" : "Copy command"}</Button
        >
      </div>
    </div>
  {:else}
    <div class="flex flex-wrap gap-2">
      <Button onclick={() => edit({ kind: "add" })}>Add machine</Button>
      <Button variant="quiet" onclick={() => edit({ kind: "fleet" })}>Import DEVBOXES_JSON</Button>
      <Button variant="quiet" onclick={() => edit({ kind: "sandbox" })}>Launch sandbox</Button>
    </div>
  {/if}
</Dialog>
