<script lang="ts">
  import { untrack } from "svelte";
  import { editableRecoveryPart, semantic, type RecoveryDraft } from "./semantic.svelte";
  import Button from "./ui/Button.svelte";
  import Field from "./ui/Field.svelte";
  import { Textarea } from "./ui/textarea";

  let { draft, disabled }: { draft: RecoveryDraft; disabled: boolean } = $props();
  let text = $state(
    untrack(() =>
      draft.shell === undefined
        ? draft.input.filter(editableRecoveryPart).map((part) => part.text)
        : [draft.shell],
    ),
  );
  const attachments = $derived(draft.input.some((part) => !editableRecoveryPart(part)));
</script>

<form
  class="grid gap-2 pt-1"
  onsubmit={(event) => {
    event.preventDefault();
    void semantic.recover(draft.id, "retry", $state.snapshot(text));
  }}
>
  {#each text as _, index (index)}
    <Field
      label={draft.shell !== undefined
        ? "Shell command"
        : text.length === 1
          ? "Message"
          : `Message text ${index + 1}`}
    >
      <Textarea rows={1} bind:value={text[index]} {disabled} />
    </Field>
  {/each}
  {#if attachments}
    <p class="text-small text-muted">Original attachments are retained.</p>
  {/if}
  <div class="flex justify-end gap-2">
    <Button variant="quiet" {disabled} onclick={() => semantic.recover(draft.id, "discard")}
      >Discard</Button
    >
    <Button type="submit" {disabled}>{draft.queued ? "Save to queue" : "Send"}</Button>
  </div>
</form>
