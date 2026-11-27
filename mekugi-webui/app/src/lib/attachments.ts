import type { Machine } from "./hub.svelte";
import type { Attachment, ThreadStartResponse } from "./protocol";
import type { SelectedSkill } from "./semantic.svelte";

export type AttachmentPathTarget = { machine: Machine; thread?: string; workspace?: string };

export async function hostPathAttachment(
  { machine, thread, workspace }: AttachmentPathTarget,
  path: string,
  id: string,
  signal: AbortSignal,
): Promise<PendingAttachment> {
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

export type PendingAttachment = { id: string; placeholder?: string } & (
  | { file: File; reference?: string; image?: true }
  | { path: string; fileReference?: boolean }
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
  if (/[ \t"'`()[\]{}<>,;]/.test(path) || /[.,:!?]$/.test(path) || /^@|#L/.test(path)) {
    return "@" + (path.includes("'") ? JSON.stringify(path) : "'" + path + "'");
  }
  return "@" + path;
}

export function referenceLabel(item: PendingAttachment): string | undefined {
  return "file" in item && item.reference !== undefined
    ? (item.placeholder ?? `[Selected ${item.file.name}]`)
    : undefined;
}

export function attachmentPlaceholder(item: PendingAttachment): string | undefined {
  return item.placeholder ?? referenceLabel(item);
}

export function attachmentRanges(text: string, items: PendingAttachment[]) {
  const ranges: { start: number; end: number }[] = [];
  for (const item of items) {
    const label = attachmentPlaceholder(item);
    if (!label) continue;
    for (
      let start = text.indexOf(label);
      start >= 0;
      start = text.indexOf(label, start + label.length)
    )
      ranges.push({ start, end: start + label.length });
  }
  return ranges;
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
    for (const range of attachmentRanges(value, draft().items)) {
      if (start === end && !deleting && range.start < start && start < range.end) {
        start = end = range.end;
        continue;
      }
      if (
        (range.start < end && start < range.end) ||
        (start === end &&
          ((start === range.end && inputType.endsWith("Backward")) ||
            (start === range.start && inputType.endsWith("Forward"))))
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
  field.addEventListener(
    "cut",
    () => {
      if (field.selectionStart !== field.selectionEnd) widenSelection("deleteByCut");
    },
    options,
  );
  field.addEventListener(
    "keydown",
    (event) => {
      if (event.isComposing || event.altKey || event.ctrlKey || event.metaKey) return;
      const backwards = event.key === "ArrowLeft";
      if ((!backwards && event.key !== "ArrowRight") || field.selectionStart !== field.selectionEnd)
        return;
      const caret = field.selectionStart;
      const range = attachmentRanges(field.value, draft().items).find(({ start, end }) =>
        backwards ? start < caret && caret <= end : start <= caret && caret < end,
      );
      if (!range) return;
      event.preventDefault();
      const next = backwards ? range.start : range.end;
      field.setSelectionRange(
        event.shiftKey ? Math.min(caret, next) : next,
        event.shiftKey ? Math.max(caret, next) : next,
        backwards ? "backward" : "forward",
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
      while (start < (selection?.start ?? end) && start < nextEnd && text[start] === next[start])
        start++;
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
    if (!text.includes(mention)) return mention;
  }
  const name = ("file" in item ? item.file.name : item.path).replace(/[\r\n]/g, " ");
  const kind =
    "file" in item
      ? item.file.type.startsWith("image/")
        ? "Image"
        : "File"
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
    return !label || text.includes(label);
  });
}

export function imageAttachment(item: Extract<PendingAttachment, { file: File }>): boolean {
  return item.image || item.file.type.startsWith("image/");
}

export function uploadBytes(items: PendingAttachment[]): number {
  return items.reduce((size, item) => size + ("file" in item ? item.file.size : 0), 0);
}

// Retain reference bytes while editing, including after deletion for browser
// undo. Only explicitly captured, uniquely present markers bind on submission.
export function inlineReferences(text: string, items: PendingAttachment[]) {
  const references = items.flatMap((item) => {
    const label = referenceLabel(item);
    const start = label ? text.indexOf(label) : -1;
    return label && start >= 0 && text.indexOf(label, start + 1) < 0
      ? [{ start, end: start + label.length, item }]
      : [];
  });
  // ES2022 has no toSorted; this array belongs to this projection.
  // oxlint-disable-next-line unicorn/no-array-sort
  return references.sort((a, b) => a.start - b.start);
}

export function submissionAttachments(
  text: string,
  items: PendingAttachment[],
): PendingAttachment[] {
  return activeAttachments(text, items).filter((item) => {
    const label = attachmentPlaceholder(item);
    if (!label) return true;
    const start = text.indexOf(label);
    if (text.indexOf(label, start + 1) >= 0)
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
