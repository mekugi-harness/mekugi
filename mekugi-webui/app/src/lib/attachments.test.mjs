import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import {
  activeAttachments,
  labelAttachment,
  prepareAttachments,
  submissionAttachments,
  textUploadFrames,
  textUploadSnapshot,
  uploadLimit,
} from "./attachments.ts";

const originalFileReader = globalThis.FileReader;
let readers;
// A controllable FileReader boundary. Payloads come from actual File bytes;
// events remain held so cancellation and in-flight progress can be observed.
class Reader extends EventTarget {
  result = null;
  readAsDataURL(file) {
    this.file = file;
    readers.push(this);
  }
  readAsArrayBuffer(file) {
    this.buffer = true;
    this.readAsDataURL(file);
  }
  progress(loaded) {
    const event = new Event("progress");
    Object.defineProperty(event, "loaded", { value: loaded });
    this.dispatchEvent(event);
  }
  async finish() {
    const data = await this.file.arrayBuffer();
    this.result = this.buffer
      ? data
      : `data:${this.file.type};base64,${Buffer.from(data).toString("base64")}`;
    this.dispatchEvent(new Event("load"));
    this.dispatchEvent(new Event("loadend"));
  }
  abort() {
    this.dispatchEvent(new Event("abort"));
    this.dispatchEvent(new Event("loadend"));
  }
}
beforeEach(() => {
  readers = [];
  globalThis.FileReader = Reader;
});
afterEach(() => {
  globalThis.FileReader = originalFileReader;
});
const tick = () => new Promise((resolve) => setImmediate(resolve));

test("a reference keeps its selected bytes and provenance separate from uploads", async () => {
  const result = await prepareAttachments(
    [{ id: "selection", file: new File(["selected 日本語"], "output"), reference: "source.go" }],
    new AbortController().signal,
    () => {},
  );
  assert.equal(readers.length, 0);
  assert.deepEqual(result, [
    {
      source: "selection",
      name: "output",
      placeholder: "[Selected output]",
      path: "source.go",
      inline: true,
      data: "selected 日本語",
    },
  ]);
});

test("inline references follow moved markers, retain deleted bytes for undo and refuse duplicates", () => {
  const items = [
    { id: "reference", file: new File(["quoted"], "message"), reference: "" },
    { id: "file", path: "notes.md" },
  ];
  const text = "Why [Selected message] now?";
  assert.deepEqual(submissionAttachments("Why now?", items), [items[1]]);
  assert.deepEqual(activeAttachments("Why now?", items), [items[1]]);
  assert.deepEqual(activeAttachments(text, items), items);
  assert.deepEqual(submissionAttachments(text, items), items);
  assert.throws(() => submissionAttachments(text + " [Selected message]", items), /more than once/);
});

test("uploads and host paths each get a unique marker and only present markers are sent", () => {
  const upload = { id: "photo", file: new File(["photo"], "shot.png", { type: "image/png" }) };
  const file = { id: "file", path: "notes.md", fileReference: true };
  const photo = { ...upload, placeholder: labelAttachment(upload, "") };
  const notes = { ...file, placeholder: labelAttachment(file, "") };
  assert.equal(photo.placeholder, "[Image shot.png]");
  assert.equal(notes.placeholder, "@'notes.md'");
  assert.equal(labelAttachment(upload, photo.placeholder), "[Image shot.png 2]");
  assert.deepEqual(submissionAttachments(`see ${notes.placeholder}`, [photo, notes]), [notes]);
  assert.deepEqual(submissionAttachments("@notes.md.bak @'notes.md'.next", [notes]), [notes]);
  assert.deepEqual(activeAttachments("@notes.md.bak", [notes]), []);
  assert.deepEqual(activeAttachments("@'notes.md'#L1", [notes]), [notes]);
});

test("mixed File and path inputs retain order, bytes and cumulative progress", async () => {
  const progress = [];
  const result = prepareAttachments(
    [
      { id: "one", path: "before.txt" },
      { id: "two", file: new File(["hi"], "notes.txt", { type: "text/plain" }) },
      { id: "three", path: "middle.txt" },
      {
        id: "four",
        file: new File([new Uint8Array([0, 255, 1])], "photo.png", { type: "image/png" }),
      },
      { id: "five", path: "after.txt" },
    ],
    new AbortController().signal,
    (read, total) => progress.push([read, total]),
  );
  assert.equal(readers.length, 1, "read one file at a time");
  readers[0].progress(1);
  await readers[0].finish();
  await tick();
  assert.equal(readers.length, 2);
  readers[1].progress(1);
  await readers[1].finish();
  assert.deepEqual(await result, [
    { source: "path", path: "before.txt" },
    { source: "textUpload", name: "notes.txt", data: "hi" },
    { source: "path", path: "middle.txt" },
    { source: "upload", name: "photo.png", mime: "image/png", data: "AP8B" },
    { source: "path", path: "after.txt" },
  ]);
  assert.deepEqual(progress, [
    [0, 5],
    [1, 5],
    [2, 5],
    [3, 5],
    [5, 5],
  ]);
});

test("32 MiB total is admitted, excess is refused before reading", async () => {
  const controller = new AbortController();
  const boundary = new File([new Uint8Array(uploadLimit)], "boundary.png", { type: "image/png" });
  const progress = [];
  const result = prepareAttachments(
    [{ id: "one", file: boundary }],
    controller.signal,
    (read, total) => progress.push([read, total]),
  );
  assert.equal(readers.length, 1);
  assert.deepEqual(progress, [[0, 32 * 1024 * 1024]]);
  controller.abort();
  await assert.rejects(result, { name: "AbortError" });
  await assert.rejects(
    prepareAttachments(
      [
        { id: "one", file: boundary },
        { id: "two", file: new File(["x"], "excess.txt") },
      ],
      new AbortController().signal,
      () => assert.fail("oversized input must not report reading"),
    ),
    /32 MiB/,
  );
  assert.equal(readers.length, 1);
});

test("binary uploads cannot become text and UTF-8 snapshots split only between characters", async () => {
  const file = new File([new Uint8Array([0, 255, 1])], "binary.dat");
  const invalid = prepareAttachments([{ id: "bad", file }], new AbortController().signal, () => {});
  await readers[0].finish();
  await assert.rejects(invalid, /require UTF-8 without NUL bytes/);

  const text = "a".repeat(24 * 1024 - 1) + "界tail";
  const item = { source: "textUpload", name: "file.txt", data: text };
  const snapshot = textUploadSnapshot([item], "snapshot-id");
  const frames = textUploadFrames(snapshot);
  assert.equal(frames.length, 2);
  assert.ok(frames[1].endsWith("\n界tail"));
  assert.equal(frames.map((frame) => frame.slice(frame.indexOf("\n") + 1)).join(""), text);
  assert.throws(
    () => textUploadSnapshot([{ ...item, data: "x".repeat(192 * 1024) }], "too-large"),
    /192 KiB/,
  );
});

test("cancellation during reading prevents later files and pre-aborted paths are rejected", async () => {
  const controller = new AbortController();
  const result = prepareAttachments(
    [
      { id: "one", file: new File(["hi"], "first.txt") },
      { id: "two", file: new File(["later"], "second.txt") },
    ],
    controller.signal,
    () => {},
  );
  controller.abort();
  await assert.rejects(result, { name: "AbortError" });
  assert.equal(readers.length, 1);
  await assert.rejects(
    prepareAttachments([{ id: "path", path: "notes.txt" }], controller.signal, () => {}),
    { name: "AbortError" },
  );
});
