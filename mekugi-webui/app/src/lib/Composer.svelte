<script lang="ts">
  import Alert from "./ui/Alert.svelte";
  import { onDestroy, tick, untrack } from "svelte";
  import Attachments from "./Attachments.svelte";
  import { prepareAttachments, type InitialInput, type PendingAttachment } from "./attachments";
  import type { Machine } from "./hub.svelte";
  import type { Input } from "./protocol";
  import { semantic } from "./semantic.svelte";
  import { stream } from "./stream.svelte";
  import ThreadSettings from "./ThreadSettings.svelte";
  import Button from "./ui/Button.svelte";
  import ComposerCard from "./ui/ComposerCard.svelte";
  import * as InputGroup from "./ui/input-group";
  import Menu from "./ui/Menu.svelte";
  import MenuItem from "./ui/MenuItem.svelte";
  import Tag from "./ui/Tag.svelte";
  import { composerText, composerTools } from "./ui/styles";

  let {
    machine,
    thread,
    input,
    agent = "/root",
    role,
    initial,
    oninitial,
    asking = false,
    unavailable,
  }: {
    machine: Machine;
    thread: string;
    input?: Input;
    agent?: string;
    role?: string;
    initial?: InitialInput;
    oninitial: () => void;
    asking?: boolean;
    unavailable?: string;
  } = $props();

  let editor = $derived(semantic.editor(thread));
  let text = $state("");
  let items = $state<PendingAttachment[]>([]);
  let pathing = $state(false);
  let sending = $state(false);
  let progress = $state("");
  let error = $state<string>();
  let tray = $state<ReturnType<typeof Attachments>>();
  let field = $state<HTMLTextAreaElement>();
  let controller = $state<AbortController>();
  const touch = matchMedia("(pointer: coarse)").matches;

  $effect(() => {
    const current = editor;
    untrack(() => {
      text = current.text;
      items = current.items;
    });
  });

  $effect(() => {
    editor.text = text;
    editor.items = items;
  });

  // Deliver the first message entered on the New Thread screen once connected.
  $effect(() => {
    const first = initial;
    if (!first || semantic.connection !== "open" || unavailable) return;
    untrack(() => {
      oninitial();
      void send(first.text, first.attachments);
    });
  });

  onDestroy(() => {
    controller?.abort();
  });

  const disabled = $derived(!!unavailable || sending || semantic.connection !== "open");
  const stoppable = $derived(!!input?.stoppable || !!stream.activeTurnId);
  const canSend = $derived(!disabled && (!!text.trim() || !!items.length));

  const SLASH_COMMANDS = [
    { name: "/compact", description: "Compact context now", insert: "/compact" },
    { name: "/archive", description: "Archive this session", insert: "/archive" },
  ];

  let slashDismissed = $state(false);
  let slashIndex = $state(0);

  const slashQuery = $derived.by(() => {
    if (!text.startsWith("/") || /\s/.test(text) || text.indexOf("/", 1) >= 0) return undefined;
    return text.slice(1).toLowerCase();
  });
  const slashMatches = $derived.by(() => {
    if (slashQuery === undefined || slashDismissed) return [];
    if (!slashQuery) return SLASH_COMMANDS;
    const prefix = SLASH_COMMANDS.filter((c) => c.name.slice(1).startsWith(slashQuery));
    const substr = SLASH_COMMANDS.filter(
      (c) => !c.name.slice(1).startsWith(slashQuery) && c.name.slice(1).includes(slashQuery),
    );
    return [...prefix, ...substr];
  });

  $effect(() => {
    void slashQuery;
    slashDismissed = false;
    slashIndex = 0;
  });

  function pickSlash(cmd: (typeof SLASH_COMMANDS)[number]) {
    text = cmd.insert;
    slashDismissed = true;
    void tick().then(() => {
      if (!field) return;
      field.focus();
      field.setSelectionRange(text.length, text.length);
    });
  }

  async function send(
    overrideText?: string,
    overrideAttachments?: import("./protocol").Attachment[],
    queue = false,
  ) {
    if (sending) return;
    const bodyText = overrideText ?? text;
    if (!bodyText.trim() && !(overrideAttachments?.length ?? items.length)) return;
    const pending = new AbortController();
    controller = pending;
    sending = true;
    error = undefined;
    try {
      const attachments =
        overrideAttachments ??
        (await prepareAttachments(items, pending.signal, (read, total) => {
          progress = total
            ? `Reading attachments: ${Math.round((read / total) * 100)}%`
            : "Preparing input…";
        }));
      progress = "Sending…";
      const result = await semantic.sendInput(bodyText, attachments, {
        queue,
        signal: pending.signal,
      });
      if (result) {
        error = result;
        return;
      }
      if (overrideText === undefined) {
        text = "";
        items = [];
        editor.text = "";
        editor.items = [];
      }
      await tick();
      field?.focus();
    } catch (failure) {
      if (!pending.signal.aborted) {
        error = failure instanceof Error ? failure.message : "Could not send message.";
      }
    } finally {
      sending = false;
      controller = undefined;
      progress = "";
    }
  }

  async function stop() {
    error = await semantic.interrupt();
  }

  function onkeydown(event: KeyboardEvent) {
    if (slashMatches.length) {
      if (event.key === "ArrowDown") {
        event.preventDefault();
        slashIndex = (slashIndex + 1) % slashMatches.length;
        return;
      }
      if (event.key === "ArrowUp") {
        event.preventDefault();
        slashIndex = (slashIndex - 1 + slashMatches.length) % slashMatches.length;
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        slashDismissed = true;
        return;
      }
      if (event.key === "Tab") {
        event.preventDefault();
        pickSlash(slashMatches[slashIndex] ?? slashMatches[0]);
        return;
      }
      if (event.key === "Enter" && !event.shiftKey && !event.isComposing && !touch) {
        const chosen = slashMatches[slashIndex] ?? slashMatches[0];
        if (chosen && text.trim().toLowerCase() !== chosen.name) {
          event.preventDefault();
          pickSlash(chosen);
          return;
        }
      }
    }
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing && !touch) {
      event.preventDefault();
      if (canSend) void send(undefined, undefined, event.metaKey || event.ctrlKey);
    }
  }
</script>

<div class="mx-auto w-full max-w-column px-4 pt-2 pb-4 max-sm:px-2 max-sm:pb-safe-2">
  {#if error}<Alert class="mb-2">{error}</Alert>{/if}
  {#if slashMatches.length}
    <div
      role="listbox"
      aria-label="Slash commands"
      class="mb-1.5 max-h-56 overflow-y-auto rounded-card border border-border bg-surface p-1 shadow-sm"
    >
      {#each slashMatches as cmd, i (cmd.name)}
        <button
          type="button"
          role="option"
          aria-selected={i === slashIndex}
          class={[
            "flex w-full items-baseline justify-between gap-3 rounded-control px-2.5 py-1.5 text-left text-small",
            i === slashIndex ? "bg-active text-ink" : "text-ink hover:bg-hover",
          ]}
          onmousedown={(e) => {
            e.preventDefault();
            pickSlash(cmd);
          }}
        >
          <span class="font-mono font-medium">{cmd.name}</span>
          <span class="truncate text-muted">{cmd.description}</span>
        </button>
      {/each}
    </div>
  {/if}
  <ComposerCard
    accepts={!disabled}
    onfiles={(files) => tray?.add(files)}
    onsubmit={() => void send()}
  >
    <Attachments
      bind:this={tray}
      bind:items
      bind:pathing
      machine={machine.label}
      previewTarget={{ machine, thread }}
      {disabled}
    />
    <label class="sr-only" for="composer-text">Message {stream.label(agent)}</label>
    <textarea
      id="composer-text"
      bind:this={field}
      data-slot="input-group-control"
      class="{composerText} min-h-14 pt-3 pb-1.5"
      bind:value={text}
      {onkeydown}
      onpaste={(event) => {
        const files = event.clipboardData?.files;
        if (!files?.length || disabled) return;
        if (!event.clipboardData?.types.includes("text/plain")) event.preventDefault();
        tray?.add(files);
      }}
      rows="2"
      {disabled}
      placeholder={asking
        ? "Answer the prompt above, or write a follow-up…"
        : stoppable
          ? "Steer the active turn…"
          : `Message ${stream.label(agent)}`}></textarea>
    <InputGroup.Addon
      align="block-end"
      class="{composerTools} cursor-default flex-wrap gap-1 text-body font-normal select-auto"
    >
      <Menu label="Attach" icon="plus" {disabled}>
        {#snippet children(close)}
          <MenuItem
            icon="attach"
            onclick={() => {
              close();
              tray?.pick();
            }}>Attach files</MenuItem
          >
          <MenuItem
            icon="folder"
            onclick={() => {
              close();
              pathing = true;
            }}>Attach path on {machine.label}</MenuItem
          >
        {/snippet}
      </Menu>
      <ThreadSettings {disabled} />
      {#if role}<Tag color={stream.roleColor(role)} class="ml-1.5 max-sm:hidden">{role}</Tag>{/if}
      <span class="flex-1"></span>
      {#if progress}<span class="text-small text-muted">{progress}</span>{/if}
      {#if stoppable}
        <Button
          variant="quiet"
          icon="stop"
          label="Interrupt turn"
          disabled={semantic.connection !== "open"}
          onclick={() => void stop()}
        />
      {/if}
      <Button
        type="submit"
        variant="primary"
        shape="round"
        icon="send"
        label={stoppable ? "Steer turn" : "Send message"}
        disabled={!canSend}
      />
    </InputGroup.Addon>
  </ComposerCard>
  {#if unavailable}
    <p class="mt-1.5 px-1 text-small text-muted">{unavailable}</p>
  {:else if stream.notice}
    <p class="mt-1.5 px-1 text-small text-muted">{stream.notice}</p>
  {/if}
</div>
