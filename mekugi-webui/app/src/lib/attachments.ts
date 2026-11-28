import type { Machine } from "./hub.svelte";
import type { Attachment, ThreadStartResponse } from "./protocol";
import type { SelectedSkill } from "./semantic.svelte";

export type AttachmentPathTarget = {
  machine: Machine;
  thread?: string;
  workspace?: string;
  deferUnadmitted?: boolean;
};

export async function hostPathAttachment(
  { machine, thread, workspace, deferUnadmitted }: AttachmentPathTarget,
  path: string,
  id: string,
  signal: AbortSignal,
): Promise<PendingAttachment> {
  if (deferUnadmitted && !thread) {
    signal.throwIfAborted();
    if (!path || path.startsWith("data:") || path.includes("://"))
      throw new Error("Choose an attachment path on the execution machine.");
    return { id, path, deferred: true };
  }
  const deadline = AbortSignal.timeout(10_000);
  try {
    const { mime } = await machine.resource<{ mime: string | null }>(
      "attachment-image",
      { thread, body: { path, workspace, probe: true } },
      { signal: AbortSignal.any([signal, deadline]) },
    );
    return { id, path, ...(!mime ? { fileReference: true } : {}) };
  } catch (error) {
    if (deadline.aborted && !signal.aborted)
      throw new Error("Checking the attachment timed out.", { cause: error });
    throw error;
  }
}

export type DeferredResolution =
  | { ok: true; text: string; items: PendingAttachment[] }
  | { ok: false; text: string; items: PendingAttachment[]; error: string };

export async function resolveDeferredPathAttachments(
  target: { machine: Machine; thread: string },
  text: string,
  items: PendingAttachment[],
  signal: AbortSignal,
): Promise<DeferredResolution> {
  let nextText = text;
  const nextItems = [...items];
  for (let index = 0; index < nextItems.length; index++) {
    const item = nextItems[index];
    if (!("path" in item) || !item.deferred) continue;
    let resolved: PendingAttachment;
    try {
      resolved = await hostPathAttachment(target, item.path, item.id, signal);
    } catch (failure) {
      if (signal.aborted) throw failure;
      const message = failure instanceof Error ? failure.message : "Could not check that path.";
      return { ok: false, text: nextText, items: nextItems, error: `${item.path}: ${message}` };
    }
    const oldLabel = item.placeholder;
    if (oldLabel) {
      const [at] = attachmentOffsets(nextText, oldLabel);
      if (at !== undefined) {
        const withoutOld = nextText.slice(0, at) + nextText.slice(at + oldLabel.length);
        const newLabel = labelAttachment(resolved, withoutOld);
        nextText = nextText.slice(0, at) + newLabel + nextText.slice(at + oldLabel.length);
        nextItems[index] = { ...resolved, placeholder: newLabel };
        continue;
      }
    }
    nextItems[index] = { ...resolved, ...(oldLabel ? { placeholder: oldLabel } : {}) };
  }
  return { ok: true, text: nextText, items: nextItems };
}

export type PendingAttachment = { id: string; placeholder?: string } & (
  | { file: File; reference?: string; image?: true }
  | { path: string; fileReference?: boolean; deferred?: boolean }
);
export type InitialInput = {
  started: Pick<
    ThreadStartResponse,
    "thread" | "model" | "reasoningEffort" | "serviceTier" | "activePermissionProfile"
  >;
  text: string;
  items: PendingAttachment[];
  attachments: Attachment[];
  skills?: SelectedSkill[];
  draftError?: string;
  onstarted?: (turnId: string) => void;
};
export const uploadLimit = 32 * 1024 * 1024;
export const selectionAppendix = "\n\n---\nUser selections:\n";
const fileSnapshotPrefix = "<mekugi-file-attachments-v2>\n";
const fileSnapshotSuffix = "\n</mekugi-file-attachments-v2>";
const fileSnapshotLimit = 192 * 1024;
const fileFrameBytes = 24 * 1024;
const encoder = new TextEncoder();
const snapshotDecoder = new TextDecoder("utf-8", { ignoreBOM: true });
const uploadDecoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

// Use the native request builder's existing retained attachment envelope.
export function textUploadSnapshot(attachments: Attachment[], id: string): string | undefined {
  const files = attachments.filter((item) => item.source === "textUpload");
  if (!files.length) return;
  const frames: string[] = [];
  for (const file of files) {
    const bytes = encoder.encode(file.data);
    const name = JSON.stringify(file.name);
    let start = 0;
    do {
      let end = Math.min(start + fileFrameBytes, bytes.length);
      while (end < bytes.length && (bytes[end] & 0xc0) === 0x80) end--;
      const header = `Attached file ${name} (UTF-8 bytes ${start}:${end} of ${bytes.length}; file content, not a separate request):\n`;
      frames.push(header + snapshotDecoder.decode(bytes.subarray(start, end)));
      start = end;
    } while (start < bytes.length);
  }
  const snapshot = fileSnapshotPrefix + JSON.stringify({ id, frames }) + fileSnapshotSuffix;
  if (encoder.encode(snapshot).length > fileSnapshotLimit)
    throw new Error("Combined text attachments exceed the 192 KiB snapshot limit.");
  return snapshot;
}

export function textUploadFrames(text: string): string[] {
  if (
    !text.startsWith(fileSnapshotPrefix) ||
    !text.endsWith(fileSnapshotSuffix) ||
    text.length > fileSnapshotLimit
  )
    return [];
  try {
    const snapshot = JSON.parse(text.slice(fileSnapshotPrefix.length, -fileSnapshotSuffix.length));
    return typeof snapshot?.id === "string" &&
      snapshot.id &&
      Array.isArray(snapshot.frames) &&
      snapshot.frames.every(
        (frame: unknown) => typeof frame === "string" && frame.startsWith("Attached file "),
      )
      ? snapshot.frames
      : [];
  } catch {
    return [];
  }
}

export function formatPathMention(path: string): string {
  // A closing quote separates the attachment from adjacent edits and longer paths.
  return "@" + (path.includes("'") ? JSON.stringify(path) : "'" + path + "'");
}

export function referenceLabel(item: PendingAttachment): string | undefined {
  return "file" in item && item.reference !== undefined
    ? (item.placeholder ?? `[Selected ${item.file.name}]`)
    : undefined;
}

export function attachmentPlaceholder(item: PendingAttachment): string | undefined {
  return item.placeholder ?? referenceLabel(item);
}

export function attachmentOffsets(text: string, label: string): number[] {
  const offsets: number[] = [];
  if (!label) return offsets;
  for (let at = text.indexOf(label); at >= 0; at = text.indexOf(label, at + label.length)) {
    offsets.push(at);
  }
  return offsets;
}

export function attachmentRanges(text: string, items: PendingAttachment[]) {
  return items.flatMap((item) => {
    const label = attachmentPlaceholder(item);
    return label
      ? attachmentOffsets(text, label).map((start) => ({ start, end: start + label.length }))
      : [];
  });
}

// Widen native edits before the textarea applies them so undo/redo still
// belongs to the browser. Refuse input that bypasses beforeinput and splits a token.
export function atomicAttachmentInput(
  field: HTMLTextAreaElement,
  draft: () => { text: string; items: PendingAttachment[] },
) {
  const listeners = new AbortController();
  const options = { capture: true, signal: listeners.signal };
  let before: { text: string; start: number; end: number } | undefined;

  function widenSelection(inputType: string) {
    let { selectionStart: start, selectionEnd: end } = field;
    const { selectionDirection: direction, value } = field;
    const deleting = inputType.startsWith("delete");
    const word = inputType.startsWith("deleteWord");
    const backwards = inputType.endsWith("Backward");
    const forwards = inputType.endsWith("Forward");
    const caret = start === end ? start : undefined;
    const adjacent = (edge: number, position: number) =>
      edge === position || (word && /^[ \t]*$/.test(value.slice(edge, position)));
    for (const range of attachmentRanges(value, draft().items)) {
      if (caret !== undefined && !deleting && range.start < caret && caret < range.end) {
        start = end = range.end;
        break;
      }
      if (
        (range.start < end && start < range.end) ||
        (caret !== undefined &&
          ((backwards && range.end <= caret && adjacent(range.end, caret)) ||
            (forwards && caret <= range.start && adjacent(caret, range.start))))
      ) {
        start = Math.min(start, range.start);
        end = Math.max(end, range.end);
      }
    }
    if (start !== field.selectionStart || end !== field.selectionEnd)
      field.setSelectionRange(start, end, direction);
  }

  field.addEventListener(
    "beforeinput",
    (event) => {
      if (!event.isComposing && !event.inputType.startsWith("history"))
        widenSelection(event.inputType);
      before = { text: field.value, start: field.selectionStart, end: field.selectionEnd };
    },
    options,
  );
  field.addEventListener("compositionstart", () => widenSelection("insertText"), options);
  const widenClipboard = () => {
    if (field.selectionStart !== field.selectionEnd) widenSelection("");
  };
  field.addEventListener("copy", widenClipboard, options);
  field.addEventListener("cut", widenClipboard, options);
  field.addEventListener(
    "keydown",
    (event) => {
      if (event.isComposing || event.altKey || event.ctrlKey || event.metaKey) return;
      const backwards = event.key === "ArrowLeft";
      if (!backwards && event.key !== "ArrowRight") return;
      const { selectionStart: start, selectionEnd: end, selectionDirection } = field;
      if (!event.shiftKey && start !== end) return;
      const reversed = selectionDirection === "backward";
      const caret = reversed ? start : end;
      const range = attachmentRanges(field.value, draft().items).find((attachment) =>
        backwards
          ? attachment.start < caret && caret <= attachment.end
          : attachment.start <= caret && caret < attachment.end,
      );
      if (!range) return;
      event.preventDefault();
      const next = backwards ? range.start : range.end;
      const anchor = event.shiftKey ? (reversed ? end : start) : next;
      field.setSelectionRange(
        Math.min(anchor, next),
        Math.max(anchor, next),
        next < anchor ? "backward" : "forward",
      );
    },
    options,
  );
  field.addEventListener(
    "input",
    (event) => {
      const prior = before;
      before = undefined;
      const { text, items } = draft();
      const next = field.value;
      if (!items.length || text === next || event.inputType.startsWith("history")) return;
      // Keep the selected range in the edit even when its replacement repeats
      // a common prefix/suffix. For browser-generated edits, recover the changed
      // interval from the nearest unchanged text.
      let start = 0;
      let end = text.length;
      let nextEnd = next.length;
      const selection = prior?.text === text ? prior : undefined;
      const prefixEnd = event.inputType.startsWith("delete")
        ? Math.min(selection?.start ?? end, field.selectionStart)
        : (selection?.start ?? end);
      while (start < prefixEnd && start < nextEnd && text[start] === next[start]) start++;
      while (
        end > (selection?.end ?? start) &&
        nextEnd > start &&
        text[end - 1] === next[nextEnd - 1]
      ) {
        end--;
        nextEnd--;
      }
      if (
        attachmentRanges(text, items).some(
          (range) =>
            range.start < end && start < range.end && (start > range.start || end < range.end),
        )
      ) {
        field.value = text;
        field.setSelectionRange(selection?.start ?? start, selection?.end ?? end);
      }
    },
    options,
  );

  return { destroy: () => listeners.abort() };
}

export function labelAttachment(item: PendingAttachment, text: string): string {
  if ("path" in item && item.fileReference) {
    const mention = formatPathMention(item.path);
    if (!attachmentOffsets(text, mention).length) return mention;
  }
  const name = ("file" in item ? item.file.name : item.path).replace(/[\r\n]/g, " ");
  const kind =
    "file" in item
      ? item.file.type.startsWith("image/")
        ? "Image"
        : "File"
      : item.deferred
        ? "Path"
        : item.fileReference
          ? "File"
          : "Image";
  let label = `[${kind} ${name}]`;
  for (let number = 2; text.includes(label); number++) label = `[${kind} ${name} ${number}]`;
  return label;
}

export function activeAttachments(text: string, items: PendingAttachment[]): PendingAttachment[] {
  return items.filter((item) => {
    const label = attachmentPlaceholder(item);
    return !label || attachmentOffsets(text, label).length > 0;
  });
}

export function imageAttachment(item: Extract<PendingAttachment, { file: File }>): boolean {
  return item.image || item.file.type.startsWith("image/");
}

export function uploadBytes(items: PendingAttachment[]): number {
  return items.reduce((size, item) => size + ("file" in item ? item.file.size : 0), 0);
}

export function submissionAttachments(
  text: string,
  items: PendingAttachment[],
): PendingAttachment[] {
  return activeAttachments(text, items).filter((item) => {
    const label = attachmentPlaceholder(item);
    if (!label) return true;
    if (attachmentOffsets(text, label).length > 1)
      throw new Error(
        `The reference ${label} appears more than once. Remove the duplicate before sending.`,
      );
    return true;
  });
}

export async function prepareAttachments(
  items: PendingAttachment[],
  signal: AbortSignal,
  progress: (read: number, total: number) => void,
): Promise<Attachment[]> {
  const total = uploadBytes(items);
  if (total > uploadLimit)
    throw new Error("Uploads exceed the 32 MiB limit. Remove files and try again.");
  let read = 0;
  progress(read, total);
  const result: Attachment[] = [];
  for (const item of items) {
    signal.throwIfAborted();
    const placeholder = attachmentPlaceholder(item);
    if ("path" in item) {
      result.push({
        source: "path",
        path: item.path,
        ...(item.fileReference ? { fileReference: true } : {}),
        ...(placeholder ? { placeholder } : {}),
      });
      continue;
    }
    if (item.reference !== undefined) {
      const data = await item.file.text();
      signal.throwIfAborted();
      result.push({
        source: "selection",
        name: item.file.name,
        placeholder,
        path: item.reference,
        inline: true,
        data,
      });
      read += item.file.size;
      progress(read, total);
      continue;
    }
    const image = imageAttachment(item);
    if (!image && item.file.size > fileSnapshotLimit)
      throw new Error(`${item.file.name} exceeds the 192 KiB text attachment limit.`);
    const attachment = await new Promise<Attachment>((resolve, reject) => {
      const reader = new FileReader();
      const abort = () => reader.abort();
      signal.addEventListener("abort", abort, { once: true });
      reader.addEventListener("loadend", () => signal.removeEventListener("abort", abort), {
        once: true,
      });
      reader.addEventListener(
        "abort",
        () => reject(new DOMException("Attachment preparation cancelled.", "AbortError")),
        { once: true },
      );
      reader.addEventListener(
        "error",
        () => reject(new Error(`Could not read ${item.file.name}.`)),
        { once: true },
      );
      reader.addEventListener("progress", (event) => progress(read + event.loaded, total));
      reader.addEventListener(
        "load",
        () => {
          const metadata = { name: item.file.name, ...(placeholder ? { placeholder } : {}) };
          if (image) {
            const data = String(reader.result).split(",", 2)[1];
            resolve({ source: "upload", ...metadata, mime: item.file.type, data });
            return;
          }
          try {
            const data = uploadDecoder.decode(reader.result as ArrayBuffer);
            if (data.includes("\0")) throw new Error("NUL byte");
            resolve({ source: "textUpload", ...metadata, data });
          } catch {
            reject(new Error(`${item.file.name}: text uploads require UTF-8 without NUL bytes.`));
          }
        },
        { once: true },
      );
      if (image) reader.readAsDataURL(item.file);
      else reader.readAsArrayBuffer(item.file);
    });
    result.push(attachment);
    read += item.file.size;
    progress(read, total);
  }
  return result;
}
