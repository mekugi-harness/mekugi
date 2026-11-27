<script lang="ts">
  import type {
    Model,
    ModelListResponse,
    PermissionProfileListResponse,
    ReasoningEffort,
    ThreadSettingsUpdateParams,
  } from "./protocol";
  import { semantic } from "./semantic.svelte";
  import { streams } from "./stream.svelte";
  import { errorText } from "./transport";
  import Picker from "./ui/Picker.svelte";
  import { settingToggle } from "./ui/select/styles";

  let { disabled = false, onerror }: { disabled?: boolean; onerror: (message?: string) => void } =
    $props();
  const stream = $derived(streams.current);
  let busy = $state(false);
  let threadModels = $state.raw<Model[] | undefined>();
  let profiles = $state.raw<PermissionProfileListResponse["data"]>([]);
  let updateError = $state<string>();
  let profileError = $state<string>();
  let loadedKey: string | undefined;
  const catalogThread = $derived(stream.session?.thread);
  const catalogWorkspace = $derived(stream.session?.workspace);

  $effect(() => {
    const target = stream.machine;
    const threadId = catalogThread;
    if (!target || target.connection !== "open" || !threadId) return;
    const key = `${target.id}:${threadId}`;
    void catalogWorkspace;
    if (loadedKey !== key) {
      loadedKey = key;
      threadModels = undefined;
      profiles = [];
      updateError = undefined;
    }
    profileError = undefined;
    const request = new AbortController();
    const { signal } = request;
    void target
      .request<ModelListResponse>("model/list", { includeHidden: false, threadId }, { signal })
      .then((res) => {
        if (!signal.aborted) threadModels = res.data ?? [];
      })
      .catch(() => {
        // Fall back to the existing machine catalog on a thread-catalog failure.
        if (!signal.aborted && !target.models.length) void target.refreshModels();
      });
    void target
      .request<PermissionProfileListResponse>("permissionProfile/list", { threadId }, { signal })
      .then((res) => {
        if (!signal.aborted) profiles = res.data;
      })
      .catch((err) => {
        if (!signal.aborted) profileError = `Could not list permissions: ${errorText(err)}`;
      });
    return () => request.abort();
  });

  const session = $derived(stream.session);
  const models = $derived(threadModels ?? stream.machine?.models ?? []);
  const currentModel = $derived(session?.model ?? "");
  const currentEffort = $derived(session?.effort ?? "");
  const currentTier = $derived(session?.serviceTier ?? "");
  const currentProfile = $derived(session?.permissionProfile ?? "");
  $effect(() => onerror(session ? (updateError ?? profileError) : undefined));
  const selectedModel = $derived(
    models.find((m) => m.id === currentModel || m.model === currentModel),
  );
  const efforts = $derived(
    selectedModel?.supportedReasoningEfforts?.map((e) => e.reasoningEffort) ?? [],
  );
  const fast = $derived(currentTier === "fast" || currentTier === "priority");
  const fastTier = $derived(
    selectedModel?.serviceTiers?.find((tier) => tier.id === "priority" || tier.id === "fast")?.id,
  );

  async function update(patch: Omit<ThreadSettingsUpdateParams, "threadId">) {
    if (busy || disabled) return;
    const key = loadedKey;
    busy = true;
    updateError = undefined;
    try {
      const result = await semantic.updateSettings(patch);
      if (loadedKey === key) updateError = result;
    } finally {
      busy = false;
    }
  }
</script>

{#if session}
  {#if models.length}
    <Picker
      label="Model"
      class="max-w-36"
      confirmed
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
    <span class="min-w-0 max-w-36 truncate px-1.5 text-small text-muted">{currentModel}</span>
  {/if}
  {#if efforts.length}
    <Picker
      label="Reasoning effort"
      class="flex-none"
      confirmed
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
  {#if profiles.length}
    <Picker
      label="Permissions"
      class="max-w-36"
      confirmed
      value={currentProfile}
      text={currentProfile || "Permissions"}
      groups={[
        {
          choices: profiles.map((profile) => ({
            value: profile.id,
            label: profile.id,
            detail: profile.description ?? undefined,
            disabled: !profile.allowed,
          })),
        },
      ]}
      disabled={disabled || busy}
      onchange={(permissions) => void update({ permissions })}
    />
  {/if}
  {#if fastTier}
    <button
      type="button"
      class={settingToggle}
      aria-pressed={fast}
      disabled={disabled || busy}
      onclick={() => void update({ serviceTier: fast ? "default" : fastTier })}
    >
      Fast
    </button>
  {/if}
{/if}
