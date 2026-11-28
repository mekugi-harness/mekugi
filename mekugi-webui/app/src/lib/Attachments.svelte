<script lang="ts">
  import Path from "./Path.svelte";
  import Alert from "./ui/Alert.svelte";
  import AttachmentPreview from "./AttachmentPreview.svelte";
  import { tick } from "svelte";
  import {
    activeAttachments,
    attachmentOffsets,
    attachmentPlaceholder,
    hostPathAttachment,
    imageAttachment,
    labelAttachment,
    referenceLabel,
    uploadLimit,
    type AttachmentPathTarget,
    type PendingAttachment,
  } from "./attachments";
  import { bytes } from "./format";
  import Button from "./ui/Button.svelte";
  import Icon from "./ui/Icon.svelte";
  import { input } from "./ui/styles";
  import { newIdentity } from "./transport";

  // The attachments a composer will send, in order: browser files and paths
  // on the execution machine. The composer opens the file picker and the path
  // field, and hands over dropped or pasted files.
  let {
    items = $bindable([]),
    text = $bindable(""),
    pathing = $bindable(false),
    disabled = false,
    questionImages = false,
    machine,
    previewTarget,
  }: {
    items?: PendingAttachment[];
    text?: string;
    pathing?: boolean;
    disabled?: boolean;
    questionImages?: boolean;
    machine?: string;
    previewTarget?: AttachmentPathTarget;
  } = $props();
  let path = $state("");
  let pathError = $state<string>();
  let pathRequest = $state<AbortController>();
  let picker = $state<HTMLInputElement>();
  let field = $state<HTMLInputElement>();
  const visible = $derived(activeAttachments(text, items).filter((item) => !referenceLabel(item)));
  const total = $derived(
    activeAttachments(text, items).reduce(
      (sum, item) => sum + ("file" in item ? item.file.size : 0),
      0,
    ),
  );

  export function pick() {
    picker?.click();
  }
  export function add(files: Iterable<File>) {
    if (questionImages) {
      // A pasted image can arrive with no MIME type; the native image loader
      // checks its bytes. Keep answer text independent of its image draft.
      items = [
        ...items,
        ...Array.from(files, (file) => ({ id: newIdentity(), file, image: true as const })),
      ];
    } else {
      for (const file of files) attach({ id: newIdentity(), file });
    }
  }
  export function checkingPath() {
    return !!pathRequest;
  }
  function attach(item: PendingAttachment) {
    const placeholder = labelAttachment(item, text);
    text += (text && !/\s$/.test(text) ? " " : "") + placeholder + " ";
    items = [...items, { ...item, placeholder }];
  }
  function remove(item: PendingAttachment) {
    const label = attachmentPlaceholder(item);
    if (label) {
      const [at] = attachmentOffsets(text, label);
      if (at !== undefined) {
        const end = at + label.length;
        text = text.slice(0, at) + text.slice(text[end] === " " ? end + 1 : end);
      }
    }
    items = items.filter((other) => other.id !== item.id);
  }
  $effect(() => {
    if (pathing) tick().then(() => field?.focus());
  });
  function closePath() {
    pathRequest?.abort();
    pathRequest = undefined;
    pathError = undefined;
    pathing = false;
  }
  $effect(() => {
    const target = previewTarget;
    if (!target) return;
    const targetMachine = target.machine;
    const thread = target.thread;
    const workspace = target.workspace;
    return () => {
      const current = previewTarget;
      if (
        !current ||
        targetMachine !== current.machine ||
        thread !== current.thread ||
        workspace !== current.workspace
      )
        closePath();
      else pathRequest?.abort();
    };
  });
  async function addPath() {
    const name = path.trim();
    const target = previewTarget;
    if (disabled || !target || !name || pathRequest) return;
    const pending = new AbortController();
    pathRequest = pending;
    pathError = undefined;
    try {
      const item = await hostPathAttachment(target, name, newIdentity(), pending.signal);
      if (pending.signal.aborted || disabled || !pathing) return;
      attach(item);
      path = "";
      closePath();
    } catch (error) {
      if (!pending.signal.aborted)
        pathError = error instanceof Error ? error.message : "Could not check that path.";
    } finally {
      if (pathRequest === pending) pathRequest = undefined;
    }
  }
  const name = (item: PendingAttachment) => ("file" in item ? item.file.name : item.path);
</script>

{#if !questionImages}
  <input
    class="sr-only"
    aria-label="Upload files"
    type="file"
    multiple
    tabindex="-1"
    bind:this={picker}
    {disabled}
    onchange={({ currentTarget }) => {
      add(currentTarget.files ?? []);
      currentTarget.value = "";
    }}
  />
{/if}
{#if visible.length || pathing}
  <div class="grid gap-2 px-2.5 pt-2.5">
    {#if visible.length}
      <ul class="flex min-w-0 flex-wrap items-center gap-1.5" aria-label="Attachments">
        {#each visible as item (item.id)}
          <li
            class="inline-flex min-h-[30px] max-w-full items-center gap-1.5 rounded-control bg-raised pr-0.5 pl-2 text-small text-muted max-sm:min-h-9"
          >
            {#if "file" in item && item.reference === undefined && imageAttachment(item)}
              <AttachmentPreview file={item.file} />
            {:else}
              {#if "path" in item && !item.fileReference && !item.deferred && previewTarget}
                <AttachmentPreview path={item.path} target={previewTarget} />
              {/if}
              <Icon name={"file" in item ? "attach" : "folder"} size={14} />
            {/if}
            <span
              class={["min-w-0 truncate text-ink", "path" in item && "font-mono"]}
              title={name(item)}><Path path={name(item)} /></span
            >
            {#if "file" in item}<span class="tabular-nums">{bytes(item.file.size)}</span>{/if}
            <button
              type="button"
              class="inline-flex size-6 shrink-0 items-center justify-center rounded-chip text-muted hover:enabled:bg-active hover:enabled:text-ink max-sm:size-8"
              {disabled}
              aria-label={`Remove ${name(item)}`}
              onclick={() => remove(item)}><Icon name="close" size={14} /></button
            >
          </li>
        {/each}
      </ul>
      {#if total > uploadLimit}<Alert>
          Uploads total {bytes(total)}; one message carries at most 32 MiB.
        </Alert>{/if}
    {/if}
    {#if pathing && previewTarget}
      <div class="flex gap-1.5">
        <label class="sr-only" for="attach-path">Path on {machine}</label>
        <input
          id="attach-path"
          class="{input} font-mono"
          bind:this={field}
          bind:value={path}
          {disabled}
          readonly={!!pathRequest}
          placeholder="Path on {machine}"
          autocomplete="off"
          spellcheck="false"
          onkeydown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              void addPath();
            } else if (event.key === "Escape") {
              event.preventDefault();
              closePath();
            }
          }}
        />
        <Button disabled={disabled || !path.trim() || !!pathRequest} onclick={addPath}
          >{pathRequest ? "Checking…" : "Attach path"}</Button
        >
        <Button variant="quiet" icon="close" label="Cancel path" onclick={closePath} />
      </div>
      {#if pathError}<Alert>{pathError}</Alert>{/if}
    {/if}
  </div>
{/if}
