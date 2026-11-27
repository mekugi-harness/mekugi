<script lang="ts">
  import Trash2Icon from "@lucide/svelte/icons/trash-2";
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

  type Slot<P = ComposerPreset | null> = { index: number; preset: P };
  type View =
    | { kind: "list" }
    | { kind: "edit"; slot: Slot }
    | { kind: "confirm"; slot: Slot<ComposerPreset>; action: "replace" | "delete" };

  const slotNumbers = [1, 2, 3, 4, 5, 6, 7, 8, 9, 0];
  let view: View = $state({ kind: "list" });
  let body = $state<HTMLDivElement>();
  let draft = $state("");
  let label = $state("");
  let pending = $state(false);
  let error = $state<string>();

  const frozen = $derived(disabled || pending);
  const slotsDisabled = $derived(frozen || loading || !!readError);
  const title = $derived.by(() => {
    switch (view.kind) {
      case "list":
        return "Presets";
      case "edit":
        return `${view.slot.preset ? "Edit" : "Add"} preset ${view.slot.index}`;
      case "confirm":
        return `${view.action === "replace" ? "Replace" : "Delete"} preset ${view.slot.index}?`;
    }
  });

  function show(next: View, focus = "[data-initial-focus]") {
    error = undefined;
    view = next;
    void tick().then(() => body?.querySelector<HTMLElement>(focus)?.focus());
  }

  function showList(index: number, action?: "edit" | "delete") {
    const button = action ? `button[data-preset-action="${action}"]` : "button";
    show({ kind: "list" }, `[data-preset-slot="${index}"] ${button}`);
  }

  function edit(slot: Slot) {
    draft = slot.preset?.input ?? seed;
    label = slot.preset?.label ?? "";
    show({ kind: "edit", slot });
  }

  function choose(index: number) {
    if (slotsDisabled || !slots) return;
    const preset = slots[index];
    if (preset) onrecall(preset.input);
    else edit({ index, preset: null });
  }

  function submit(event: SubmitEvent) {
    event.preventDefault();
    if (frozen || view.kind !== "edit") return;
    const { index, preset } = view.slot;
    if (preset && draft !== preset.input) {
      show({ kind: "confirm", action: "replace", slot: { index, preset } });
      return;
    }
    void save(view.slot, { input: draft, label });
  }

  function cancelConfirmation() {
    if (view.kind !== "confirm") return;
    if (view.action === "replace") show({ kind: "edit", slot: view.slot });
    else showList(view.slot.index, "delete");
  }

  function confirm() {
    if (view.kind !== "confirm") return;
    void save(view.slot, view.action === "delete" ? null : { input: draft, label });
  }

  async function save(slot: Slot, preset: ComposerPreset | null) {
    if (frozen) return;
    pending = true;
    error = undefined;
    const params: ThreadPresetWriteParams = {
      threadId: thread,
      slot: slot.index,
      expected: slot.preset,
      preset,
    };
    try {
      const result = await machine.request<ThreadPresetWriteResponse>(
        "thread/presets/write",
        params,
      );
      onsaved(slot.index, result.preset);
      showList(slot.index);
    } catch (failure) {
      show(preset ? { kind: "edit", slot } : view);
      error = errorText(failure, true);
    } finally {
      pending = false;
    }
  }

  function shortcut(event: KeyboardEvent) {
    if (
      view.kind !== "list" ||
      slotsDisabled ||
      !slots ||
      !body?.closest("[role=dialog]")?.contains(document.activeElement) ||
      event.defaultPrevented ||
      event.isComposing ||
      event.repeat ||
      event.ctrlKey ||
      event.shiftKey ||
      event.altKey ||
      event.metaKey ||
      !/^[0-9]$/.test(event.key)
    )
      return;
    event.preventDefault();
    choose(Number(event.key));
  }
</script>

<svelte:window onkeydown={shortcut} />

<Dialog {title} {onclose}>
  <div bind:this={body}>
    {#if view.kind === "list"}
      <p class="mb-3 text-muted">Load a preset into the composer without sending.</p>
      {#if !slots && !readError}
        <div class="grid gap-0.5" aria-busy="true" aria-label="Presets">
          {#each slotNumbers as index (index)}
            <div class="h-9 rounded-control bg-raised max-sm:h-11"></div>
          {/each}
        </div>
      {:else if slots}
        <ul class="grid gap-0.5" role="list" aria-busy={loading}>
          {#each slotNumbers as index, i (index)}
            {@const preset = slots[index]}
            {@const summary =
              preset && (preset.label.trim() || preset.input.trim()).split("\n", 1)[0]}
            <li class="flex min-w-0 items-center gap-1" data-preset-slot={index}>
              <button
                type="button"
                class="flex min-h-9 min-w-0 flex-1 cursor-pointer items-center gap-3 rounded-control px-2 py-1.5 text-left text-small hover:enabled:bg-ui-accent disabled:cursor-default max-sm:min-h-11"
                disabled={slotsDisabled}
                aria-label={preset ? `Load preset ${index}: ${summary}` : `Add preset ${index}`}
                aria-keyshortcuts={String(index)}
                title={summary}
                data-initial-focus={i === 0 && !slotsDisabled ? "" : undefined}
                onclick={() => choose(index)}
              >
                <kbd
                  class="flex size-6 shrink-0 items-center justify-center rounded-chip bg-raised font-mono text-small text-muted"
                  aria-hidden="true">{index}</kbd
                >
                <span class={preset ? "min-w-0 truncate font-medium" : "text-muted"}>
                  {preset ? summary : "Add preset"}
                </span>
              </button>
              {#if preset}
                <Button
                  variant="quiet"
                  icon="compose"
                  label={`Edit preset ${index}`}
                  tooltip
                  data-preset-action="edit"
                  disabled={slotsDisabled}
                  onclick={() => edit({ index, preset })}
                />
                <Button
                  variant="quiet"
                  shape="icon"
                  label={`Delete preset ${index}`}
                  tooltip
                  data-preset-action="delete"
                  disabled={slotsDisabled}
                  onclick={() =>
                    show({ kind: "confirm", action: "delete", slot: { index, preset } })}
                >
                  <Trash2Icon strokeWidth={1.5} aria-hidden="true" />
                </Button>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
      {#if readError}<Alert class="mt-3">{readError}</Alert>{/if}
    {:else if view.kind === "edit"}
      {@const slot = view.slot}
      <form class="grid gap-4" onsubmit={submit}>
        <Field label="Prompt or slash command">
          <textarea
            name="prompt"
            class="{input} h-40 max-h-[45dvh] resize-y font-mono"
            data-initial-focus
            aria-required="true"
            readonly={pending}
            bind:value={draft}></textarea>
        </Field>
        <Field label="Label">
          <input
            name="label"
            class={input}
            readonly={pending}
            bind:value={label}
            placeholder="First line when empty"
          />
        </Field>
        {#if error}<Alert>{error}</Alert>{/if}
        <div class="flex justify-end gap-2">
          <Button
            variant="quiet"
            disabled={pending}
            onclick={() => showList(slot.index, slot.preset ? "edit" : undefined)}>Cancel</Button
          >
          <Button type="submit" variant="primary" disabled={frozen}>
            {pending ? "Saving…" : "Save"}
          </Button>
        </div>
      </form>
    {:else}
      {@const action = view.action === "replace" ? "Replace" : "Delete"}
      <div class="grid gap-3">
        <div>
          <p class="text-small text-muted">Current prompt</p>
          <p class="max-h-28 overflow-y-auto whitespace-pre-wrap wrap-anywhere">
            {view.slot.preset.input}
          </p>
        </div>
        {#if view.action === "replace"}
          <div>
            <p class="text-small text-muted">Replacement prompt</p>
            <p class="max-h-28 overflow-y-auto whitespace-pre-wrap wrap-anywhere">{draft}</p>
          </div>
        {/if}
        {#if error}<Alert>{error}</Alert>{/if}
        <!-- Keep confirmation clear of the right-aligned actions that open it. -->
        <div class="flex gap-2">
          <Button variant="quiet" data-initial-focus disabled={pending} onclick={cancelConfirmation}
            >Cancel</Button
          >
          <Button variant="danger" disabled={frozen} onclick={confirm}
            >{pending ? "Saving…" : `${action} preset ${view.slot.index}`}</Button
          >
        </div>
      </div>
    {/if}
  </div>
</Dialog>
