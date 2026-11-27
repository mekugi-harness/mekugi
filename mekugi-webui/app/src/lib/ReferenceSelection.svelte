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
  let selecting = false;
  let button = $state<HTMLButtonElement>();

  function measure() {
    const selection = document.getSelection();
    if (
      selecting ||
      !root ||
      !selection?.rangeCount ||
      !root.contains(selection.anchorNode) ||
      !root.contains(selection.focusNode) ||
      !selection.focusNode
    ) {
      selected = undefined;
      return;
    }
    const text = selection.toString();
    if (!text.trim()) {
      selected = undefined;
      return;
    }

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
    const height = button?.offsetHeight ?? 44;
    const edge = gap + (button?.offsetWidth ?? 104) / 2;
    const top = rect.top >= height + 2 * gap ? rect.top - height - gap : rect.bottom + gap;
    selected = {
      text,
      left: Math.max(edge, Math.min(innerWidth - edge, rect.left)),
      top: Math.max(gap, Math.min(innerHeight - height - gap, top)),
    };
  }

  $effect(() => {
    if (button) measure();
  });

  function capture() {
    const selection = selected;
    if (selection && onreference(selection.text, description, source)) {
      document.getSelection()?.removeAllRanges();
      selected = undefined;
    }
  }
</script>

<svelte:window onresize={measure} />
<svelte:document
  onselectionchange={measure}
  onpointerdown={(event) => {
    if (event.button === 0 && !button?.contains(event.target as Node)) {
      selecting = true;
      selected = undefined;
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
  onscrollcapture={measure}
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
