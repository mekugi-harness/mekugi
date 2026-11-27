<script lang="ts">
  import { tick } from "svelte";
  import type { Machine } from "./hub.svelte";
  import type {
    ComposerPreset,
    ThreadPresetWriteParams,
    ThreadPresetWriteResponse,
    ThreadPresetsListResponse,
  } from "./protocol";
  import { errorText } from "./transport";
  import Alert from "./ui/Alert.svelte";
  import Button from "./ui/Button.svelte";
  import Dialog from "./ui/Dialog.svelte";
  import Field from "./ui/Field.svelte";
  import { input } from "./ui/styles";

  let {
    machine,
    thread,
    seed,
    slots,
    loading,
    readError,
    disabled,
    onclose,
    onrecall,
    onsaved,
  }: {
    machine: Machine;
    thread: string;
    seed: string;
    slots?: ThreadPresetsListResponse["presets"];
    loading: boolean;
    readError?: string;
    disabled: boolean;
    onclose: () => void;
    onrecall: (input: string) => void;
    onsaved: (slot: number, preset: ComposerPreset | null) => void;
  } = $props();

  type Slot = { index: number; preset: ComposerPreset | null };
  type Action = "save" | "edit" | "rename" | "delete";
  type Step =
    | { kind: "quick" }
    | { kind: "slots"; action: Action }
    | { kind: "input"; slot?: Slot }
    | { kind: "label"; slot?: Slot; rename?: boolean }
    | { kind: "confirm"; slot: Slot; deleting?: boolean };

  const slotNumbers = [1, 2, 3, 4, 5, 6, 7, 8, 9, 0];
  const slotTitles = {
    save: "Save preset to slot",
    edit: "Edit a preset",
    rename: "Rename a preset",
    delete: "Delete a preset",
  };
  const actions = ["edit", "rename", "delete"] as const;
  let step: Step = $state({ kind: "quick" });
  let body = $state<HTMLDivElement>();
  let draft = $state("");
  let label = $state("");
  let pending = $state(false);
  let error = $state<string>();
  const listedSlots = $derived(
    slotNumbers.filter(
      (index) => step.kind !== "slots" || step.action === "save" || slots?.[index],
    ),
  );
  const frozen = $derived(disabled || pending);
  const slotsDisabled = $derived(frozen || loading || !!readError);
  const draftTitle = $derived.by(() => {
    switch (step.kind) {
      case "quick":
        return "Presets";
      case "slots":
        return slotTitles[step.action];
      case "input":
        return step.slot?.preset ? `Edit preset ${step.slot.index}` : "New preset";
      case "label":
        return step.slot
          ? `${step.rename ? "Rename" : "Label"} preset ${step.slot.index}`
          : "Label new preset";
      case "confirm":
        return `${step.deleting ? "Delete" : "Replace"} preset ${step.slot.index}?`;
    }
  });

  function enter(next: Step) {
    error = undefined;
    step = next;
    void tick().then(() => body?.querySelector<HTMLElement>("[data-initial-focus]")?.focus());
  }

  function edit(slot?: Slot) {
    draft = slot?.preset?.input ?? seed;
    label = slot?.preset?.label ?? "";
    enter({ kind: "input", slot });
  }

  function choose(index: number) {
    if (slotsDisabled || !slots) return;
    const slot = { index, preset: slots[index] ?? null };
    if (step.kind === "quick") {
      if (slot.preset) onrecall(slot.preset.input);
      else edit(slot);
      return;
    }
    if (step.kind !== "slots") return;
    switch (step.action) {
      case "save":
        if (slot.preset) enter({ kind: "confirm", slot });
        else void save(slot);
        break;
      case "edit":
        edit(slot);
        break;
      case "rename":
        if (!slot.preset) break;
        draft = slot.preset.input;
        label = slot.preset.label;
        enter({ kind: "label", slot, rename: true });
        break;
      case "delete":
        enter({ kind: "confirm", slot, deleting: true });
        break;
    }
  }

  function advance(event: SubmitEvent) {
    event.preventDefault();
    if (frozen) return;
    if (step.kind === "input") {
      if (!draft.trim()) error = "Preset input cannot be empty.";
      else if (draft.trimStart().startsWith("!"))
        error = "Presets support prompts and slash commands, not shell commands.";
      else if (new TextEncoder().encode(draft).length > 8000)
        error = "Preset input cannot exceed 8,000 UTF-8 bytes.";
      else enter({ kind: "label", slot: step.slot });
    } else if (step.kind === "label") {
      if (!step.slot) enter({ kind: "slots", action: "save" });
      else if (!step.rename && step.slot.preset) enter({ kind: "confirm", slot: step.slot });
      else void save(step.slot);
    }
  }

  async function save(slot: Slot, deleting = false) {
    if (frozen) return;
    pending = true;
    error = undefined;
    const params: ThreadPresetWriteParams = {
      threadId: thread,
      slot: slot.index,
      expected: slot.preset,
      preset: deleting ? null : { input: draft, label: label.trim() },
    };
    try {
      const result = await machine.request<ThreadPresetWriteResponse>(
        "thread/presets/write",
        params,
      );
      onsaved(slot.index, result.preset);
      enter({ kind: "quick" });
    } catch (failure) {
      error = errorText(failure);
    } finally {
      pending = false;
    }
  }

  function shortcut(event: KeyboardEvent) {
    if (
      (step.kind !== "quick" && step.kind !== "slots") ||
      slotsDisabled ||
      !slots ||
      !body?.closest("[role=dialog]")?.contains(document.activeElement) ||
      event.defaultPrevented ||
      event.isComposing ||
      event.repeat ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey ||
      event.metaKey
    )
      return;
    if (/^[0-9]$/.test(event.key) && listedSlots.includes(Number(event.key))) {
      event.preventDefault();
      choose(Number(event.key));
    } else if (step.kind === "quick") {
      if (event.key === "n") {
        event.preventDefault();
        edit();
      } else {
        const action = actions.find((choice) => choice[0] === event.key);
        if (!action || !Object.keys(slots).length) return;
        event.preventDefault();
        enter({ kind: "slots", action });
      }
    }
  }
</script>

<svelte:window onkeydown={shortcut} />

<Dialog title={draftTitle} {onclose}>
  <div bind:this={body}>
    {#if step.kind === "quick" || step.kind === "slots"}
      {#if step.kind === "quick"}
        <p class="mb-3 text-muted">Load a preset into the composer without sending.</p>
      {/if}
      {#if !slots && !readError}
        <div
          class="grid grid-cols-2 gap-2 max-sm:grid-cols-1"
          aria-busy="true"
          aria-label="Presets"
        >
          {#each slotNumbers as index (index)}
            <div class="h-9 rounded-control bg-raised max-sm:h-11"></div>
          {/each}
        </div>
      {:else if slots}
        <ul
          class="grid grid-cols-1 gap-x-2 gap-y-0.5 sm:grid-flow-col sm:grid-cols-2 sm:[grid-template-rows:var(--preset-slot-rows)]"
          style:--preset-slot-rows={`repeat(${Math.max(1, Math.ceil(listedSlots.length / 2))}, auto)`}
        >
          {#each listedSlots as index, i (index)}
            {@const stored = slots[index]}
            <li>
              <button
                type="button"
                class="flex min-h-9 w-full cursor-pointer items-center gap-3 rounded-control px-2 py-1.5 text-left text-small hover:enabled:bg-ui-accent disabled:cursor-default max-sm:min-h-11"
                disabled={slotsDisabled}
                data-initial-focus={i === 0 && !slotsDisabled ? "" : undefined}
                onclick={() => choose(index)}
              >
                <kbd
                  class="flex size-6 shrink-0 items-center justify-center rounded-chip bg-raised font-mono text-small text-muted"
                  >{index}</kbd
                >
                <span class={stored ? "min-w-0 truncate font-medium" : "text-muted"}>
                  {stored
                    ? (stored.label.trim() || stored.input.trim()).split("\n", 1)[0]
                    : "Empty"}
                </span>
              </button>
            </li>
          {/each}
        </ul>
        {#if step.kind === "quick"}
          <div class="mt-4 flex flex-wrap justify-end gap-2">
            <Button disabled={slotsDisabled} onclick={() => edit()}>New preset</Button>
            {#each actions as action (action)}
              <Button
                variant="quiet"
                disabled={slotsDisabled || !Object.keys(slots).length}
                onclick={() => enter({ kind: "slots", action })}>{slotTitles[action]}</Button
              >
            {/each}
          </div>
        {:else}
          <div class="mt-4 flex justify-end">
            <Button variant="quiet" disabled={pending} onclick={() => enter({ kind: "quick" })}
              >Cancel</Button
            >
          </div>
        {/if}
      {/if}
    {:else if step.kind === "input" || step.kind === "label"}
      <form class="grid gap-4" onsubmit={advance}>
        {#if step.kind === "input"}
          <Field label="Prompt or slash command">
            <textarea
              class="{input} min-h-40 resize-y font-mono"
              data-initial-focus
              bind:value={draft}
              onkeydown={(event) => {
                if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
                  event.preventDefault();
                  event.currentTarget.form?.requestSubmit();
                }
              }}></textarea>
          </Field>
        {:else}
          <Field label="Label">
            <input
              class={input}
              data-initial-focus
              bind:value={label}
              placeholder="First line when empty"
            />
          </Field>
        {/if}
        <div class="flex justify-end gap-2">
          <Button variant="quiet" disabled={pending} onclick={() => enter({ kind: "quick" })}
            >Cancel</Button
          >
          <Button type="submit" variant="primary" disabled={frozen}>
            {pending
              ? "Saving…"
              : step.kind === "label" && (step.rename || (step.slot && !step.slot.preset))
                ? "Save"
                : "Continue"}
          </Button>
        </div>
      </form>
    {:else if step.kind === "confirm"}
      <p class="max-h-40 overflow-y-auto whitespace-pre-wrap wrap-anywhere">
        {step.slot.preset?.input}
      </p>
      {#if !step.deleting}
        <p class="mt-2 max-h-40 overflow-y-auto whitespace-pre-wrap wrap-anywhere">→ {draft}</p>
      {/if}
      <div class="mt-4 flex justify-end gap-2">
        <Button
          variant="quiet"
          data-initial-focus
          disabled={pending}
          onclick={() => enter({ kind: "quick" })}>Cancel</Button
        >
        <Button
          variant="danger"
          disabled={frozen}
          onclick={() => {
            if (step.kind === "confirm") void save(step.slot, step.deleting);
          }}
          >{pending
            ? "Saving…"
            : `${step.deleting ? "Delete" : "Replace"} preset ${step.slot.index}`}</Button
        >
      </div>
    {/if}
    {#if error ?? readError}<Alert class="mt-3">{error ?? readError}</Alert>{/if}
  </div>
</Dialog>
