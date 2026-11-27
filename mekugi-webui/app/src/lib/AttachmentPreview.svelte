<script lang="ts">
  import type { AttachmentPathTarget } from "./attachments";
  let { file, path, target }: { file?: File; path?: string; target?: AttachmentPathTarget } =
    $props();
  let url = $state<string>();
  let failed = $state(false);
  let unsupported = $state(false);
  $effect(() => {
    const controller = new AbortController();
    let preview: string | undefined;
    url = undefined;
    failed = false;
    unsupported = false;
    if (file) {
      url = preview = URL.createObjectURL(file);
    } else if (path && target) {
      void target.machine
        .resource<Blob>(
          "attachment-image",
          { thread: target.thread, body: { path, workspace: target.workspace } },
          { signal: controller.signal },
        )
        .then((result) => {
          if (controller.signal.aborted) return;
          url = preview = URL.createObjectURL(result);
        })
        .catch(() => {
          if (controller.signal.aborted) return;
          failed = true;
        });
    }
    return () => {
      controller.abort();
      if (preview) URL.revokeObjectURL(preview);
    };
  });
</script>

{#if failed}
  <span class="my-1 flex size-16 shrink-0 items-center text-center text-small" role="status"
    >Preview unavailable</span
  >
{:else if url && !unsupported}
  <img
    src={url}
    alt={`Preview of ${file?.name ?? path}`}
    width="64"
    height="64"
    class="my-1 size-16 shrink-0 rounded-chip object-contain"
    onerror={() => {
      unsupported = !!file && !file.type;
      failed = !unsupported;
    }}
  />
{:else if !unsupported && (file || (target && path))}
  <span class="my-1 flex size-16 shrink-0 items-center text-center text-small" role="status"
    >Loading preview…</span
  >
{/if}
