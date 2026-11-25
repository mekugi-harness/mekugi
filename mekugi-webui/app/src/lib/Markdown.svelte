<script lang="ts">
  import { highlightWithin } from "./highlight";
  import { block, markedBlock } from "./markdown";
  import { semantic } from "./semantic.svelte";
  import { stream } from "./stream.svelte";
  import { view } from "./view.svelte";

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
  let problem = $state<string>();
  const html = $derived(spans?.length ? markedBlock(text, spans) : block(text, context));
  // Fenced code names its language as a language-NAME class.
  const fences = (rendered: string) => (node: HTMLElement) => {
    if (rendered.includes("<pre>"))
      highlightWithin(node, "pre > code", (code) => /language-(\S+)/.exec(code.className)?.[1]);
  };
  const click = (event: MouseEvent) => {
    if (!(event.target instanceof Element)) return;
    const link = event.target.closest<HTMLAnchorElement>("a[data-file-link], a[data-run-link]");
    if (!link) return;
    event.preventDefault();
    problem = undefined;
    const run = link.dataset.runLink;
    const shown = semantic.frame?.view;
    if (!shown) {
      problem = run
        ? "Run details are available on a connected live thread."
        : "File details are available on a connected live thread.";
      return;
    }
    const legacyAgent = !!view.agent && shown.Main.ID === stream.session?.thread;
    void semantic.send({
      details: {
        Kind: run ? "run" : "file",
        Path: run ?? link.dataset.fileLink ?? "",
        Source: 0,
        Output: 0,
        Sources: [],
        Agent: legacyAgent,
        Caller: "",
        Displayed: legacyAgent
          ? {
              ID:
                semantic.frame?.agents?.Roster.find((row) => row.Agent.Name === view.agent)?.Agent
                  .Thread ?? "",
              Caller: view.agent ?? "",
            }
          : shown.Displayed.ID
            ? shown.Displayed
            : shown.Main,
      },
    });
  };
  const links = (node: HTMLElement) => {
    node.addEventListener("click", click);
    return () => node.removeEventListener("click", click);
  };
</script>

<div class={["prose", quiet && "text-muted"]} {@attach fences(html)} {@attach links}>
  {@html html}
</div>

{#if problem}<p class="mt-2 text-small text-negative" role="alert">{problem}</p>{/if}
