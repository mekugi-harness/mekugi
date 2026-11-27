<script lang="ts">
  import Button from "./ui/Button.svelte";

  let {
    root,
    description = "message",
    source = "",
    onreference,
  }: {
    root: HTMLElement | undefined;
    description?: string;
    source?: string;
    onreference: (text: string, description: string, source: string) => boolean;
  } = $props();

  let selected = $state<{ text: string; left: number; top: number }>();
  // Keep tracking offscreen selections so the action returns on scroll.
  let ownsSelection = $state(false);
  let selecting = false;
  let button = $state<HTMLButtonElement>();

  function clear() {
    ownsSelection = false;
    selected = undefined;
  }

  function measure() {
    const action = button;
    const selection = document.getSelection();
    if (
      !root ||
      selecting ||
      !selection?.rangeCount ||
      !root.contains(selection.anchorNode) ||
      !root.contains(selection.focusNode) ||
      !selection.focusNode
    ) {
      clear();
      return;
    }
    const text = selection.toString();
    if (!text.trim()) {
      clear();
      return;
    }
    ownsSelection = true;

    const endpoint = document.createRange();
    endpoint.setStart(selection.focusNode, selection.focusOffset);
    const rect = endpoint.getClientRects()[0] ?? selection.getRangeAt(0).getBoundingClientRect();
    const visible = root.getBoundingClientRect();
    if (
      rect.bottom < Math.max(0, visible.top) ||
      rect.top > Math.min(innerHeight, visible.bottom)
    ) {
      selected = undefined;
      return;
    }
    const gap = 8;
    const height = action?.offsetHeight ?? 44;
    const edge = gap + (action?.offsetWidth ?? 104) / 2;
    const top = rect.top >= height + 2 * gap ? rect.top - height - gap : rect.bottom + gap;
    selected = {
      text,
      left: Math.max(edge, Math.min(innerWidth - edge, rect.left)),
      top: Math.max(gap, Math.min(innerHeight - height - gap, top)),
    };
  }

  $effect(measure);

  $effect(() => {
    if (!root || !ownsSelection) return;
    const listeners = new AbortController();
    const { signal } = listeners;
    document.addEventListener("scroll", measure, { capture: true, passive: true, signal });
    window.addEventListener("resize", measure, { signal });
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    return () => {
      listeners.abort();
      observer.disconnect();
    };
  });

  function capture() {
    const selection = selected;
    if (selection && onreference(selection.text, description, source)) {
      document.getSelection()?.removeAllRanges();
      clear();
    }
  }
</script>

<svelte:document
  onselectionchange={measure}
  onpointerdown={(event) => {
    if (event.button === 0 && !button?.contains(event.target as Node)) {
      selecting = true;
      clear();
    }
  }}
  onpointerup={() => {
    selecting = false;
    measure();
  }}
  onpointercancel={() => {
    selecting = false;
    measure();
  }}
/>

{#if selected}
  <Button
    bind:ref={button}
    size="sm"
    icon="attach"
    class="fixed z-20 -translate-x-1/2 shadow-float"
    style={`left:${selected.left}px;top:${selected.top}px`}
    onpointerdown={(event) => event.preventDefault()}
    onclick={capture}>Reference</Button
  >
{/if}
