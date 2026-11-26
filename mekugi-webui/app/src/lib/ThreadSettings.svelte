<script lang="ts">
  import { untrack } from "svelte";
  import type { Model, ModelListResponse, ReasoningEffort } from "./protocol";
  import { semantic } from "./semantic.svelte";
  import { stream } from "./stream.svelte";
  import Picker from "./ui/Picker.svelte";
  import { settingToggle } from "./ui/select/styles";

  let { disabled = false }: { disabled?: boolean } = $props();
  let busy = $state(false);
  let threadModels = $state.raw<Model[] | undefined>();
  let loadedKey = $state<string | undefined>();

  $effect(() => {
    const target = stream.machine;
    const threadId = stream.session?.thread;
    if (!target || target.connection !== "open" || !threadId) return;
    const key = `${target.id}:${threadId}`;
    if (loadedKey === key) return;
    loadedKey = key;
    threadModels = undefined;
    untrack(() => {
      void target
        .request<ModelListResponse>("model/list", { includeHidden: false, threadId })
        .then((res) => {
          if (loadedKey === key) threadModels = res.data ?? [];
        })
        .catch(() => {
          // Fall back to machine catalog if thread-scoped listing fails.
          if (!target.models.length) void target.refreshModels();
        });
    });
  });

  const session = $derived(stream.session);
  const models = $derived(threadModels ?? stream.machine?.models ?? []);
  const currentModel = $derived(session?.model ?? "");
  const currentEffort = $derived(session?.effort ?? "");
  const currentTier = $derived(session?.serviceTier ?? "");
  const selectedModel = $derived(
    models.find((m) => m.id === currentModel || m.model === currentModel),
  );
  const efforts = $derived(
    selectedModel?.supportedReasoningEfforts?.map((e) => e.reasoningEffort) ?? [],
  );
  const fast = $derived(currentTier === "fast" || currentTier === "priority");

  async function update(patch: {
    model?: string | null;
    effort?: ReasoningEffort | null;
    serviceTier?: string | null;
  }) {
    if (busy || disabled) return;
    busy = true;
    try {
      await semantic.updateSettings(patch);
    } finally {
      busy = false;
    }
  }
</script>

{#if session && (currentModel || models.length)}
  <div class="flex flex-wrap items-center gap-1">
    {#if models.length}
      <Picker
        label="Model"
        value={currentModel}
        text={selectedModel?.displayName || currentModel || "Model"}
        groups={[
          {
            choices: models
              .filter((m) => !m.hidden)
              .map((m) => ({ value: m.id || m.model, label: m.displayName || m.model || m.id })),
          },
        ]}
        disabled={disabled || busy}
        onchange={(value) => void update({ model: value })}
      />
    {:else if currentModel}
      <span class="px-1.5 text-small text-muted">{currentModel}</span>
    {/if}
    {#if efforts.length}
      <Picker
        label="Reasoning effort"
        value={currentEffort}
        text={currentEffort || selectedModel?.defaultReasoningEffort || "Effort"}
        groups={[
          {
            choices: efforts.map((value) => ({ value, label: value })),
          },
        ]}
        disabled={disabled || busy}
        onchange={(value) => void update({ effort: value as ReasoningEffort })}
      />
    {/if}
    {#if selectedModel?.serviceTiers?.length}
      <button
        type="button"
        class={settingToggle}
        aria-pressed={fast}
        disabled={disabled || busy}
        onclick={() => void update({ serviceTier: fast ? "default" : "priority" })}
      >
        Fast
      </button>
    {/if}
  </div>
{/if}
