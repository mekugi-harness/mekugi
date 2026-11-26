<script lang="ts">
  import { highlightWithin } from "./highlight";
  import { block, markedBlock } from "./markdown";

  // Rendered Markdown from recorded messages, styled by the prose component
  // class since no template declares its elements.
  let {
    text,
    quiet = false,
    spans,
    context,
  }: {
    text: string;
    quiet?: boolean;
    spans?: import("./protocol").Entry["spans"];
    context?: import("markdown-it").Env;
  } = $props();
  const html = $derived(spans?.length ? markedBlock(text, spans) : block(text, context));
  // Fenced code names its language as a language-NAME class.
  const fences = (rendered: string) => (node: HTMLElement) => {
    if (rendered.includes("<pre>"))
      highlightWithin(node, "pre > code", (code) => /language-(\S+)/.exec(code.className)?.[1]);
  };
  function onLinkClick(event: MouseEvent) {
    const target =
      event.target instanceof Element
        ? event.target.closest<HTMLElement>("[data-file-link], [data-run-link]")
        : null;
    if (!target) return;
    const value = target.dataset.fileLink || target.dataset.runLink;
    if (!value) return;
    event.preventDefault();
    if (typeof navigator !== "undefined" && navigator.clipboard?.writeText) {
      void navigator.clipboard.writeText(value).catch(() => {});
    }
    const prev = target.title;
    target.title = `Copied ${value}`;
    setTimeout(() => {
      if (target.title === `Copied ${value}`) target.title = prev;
    }, 1500);
  }
  const links = (node: HTMLElement) => {
    node.addEventListener("click", onLinkClick);
    return () => node.removeEventListener("click", onLinkClick);
  };
</script>

<div class={["prose", quiet && "text-muted"]} {@attach fences(html)} {@attach links}>
  {@html html}
</div>
