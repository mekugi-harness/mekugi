import assert from "node:assert/strict";
import { test } from "node:test";
import { detailRange, parseDetailLink } from "./detail-link.ts";
import { block, inline, referenceContext } from "./markdown.ts";

test("automatic and Markdown web links navigate safely", () => {
  const html = block(
    "https://example.com/auto [Website](https://example.com/markdown) user@example.com",
  );
  assert.match(
    html,
    /href="https:\/\/example.com\/auto" target="_blank" rel="noopener noreferrer"/,
  );
  assert.match(html, /href="https:\/\/example.com\/markdown"[^>]*>Website<\/a>/);
  assert.match(html, /href="mailto:user@example.com"/);
});

test("machine-local Markdown links retain the shared file target", () => {
  const html = block(
    "[Source](</workspace/a b.go#L12-L14>) [Relative](src/a%20b.go#L12) " +
      "[File](file:///workspace/source.go#L12) [Legacy](/workspace/source.go:12)",
  );
  assert.match(html, /<a href="#activity-details" data-file-link="\/workspace\/a b.go#L12-L14"/);
  assert.match(html, /data-file-link="src\/a b.go#L12"/);
  assert.match(html, /data-file-link="file:\/\/\/workspace\/source.go#L12"/);
  assert.match(html, /data-file-link="\/workspace\/source.go:12"/);
  assert.doesNotMatch(html, /href="#"/);
  assert.doesNotThrow(() => block("[Literal](a%ZZ.md)"));
});

test("run Markdown links retain the native detail target without navigating to the scheme", () => {
  const run = "mekugi:run/exec-00000000-0000-4000-8000-000000000042";
  const html = block(
    `[Run](${run}) [Line](${run}#L3) [Range](${run}#L3-L5) [Legacy](${run}:3-5) [Other](mekugi:other/call_42)`,
  );
  for (const suffix of ["", "#L3", "#L3-L5", ":3-5"])
    assert.ok(html.includes(`data-run-link="${run}${suffix}"`));
  assert.match(html, /href="#activity-details"/);
  assert.doesNotMatch(html, /href="mekugi:|data-file-link|data-run-link="mekugi:other/);
});

test("detail targets preserve original file and execution line ranges", () => {
  const path = "file:///workspace/a%20b.go#L12-L14";
  assert.deepEqual(parseDetailLink(path), {
    kind: "file",
    path: "/workspace/a b.go",
    label: path,
    range: { first: 12, last: 14 },
  });
  assert.equal(parseDetailLink("file://elsewhere/workspace/a.go#L12"), undefined);
  assert.equal(parseDetailLink("file:///workspace/a.go#L14-L12"), undefined);
  const itemId = "exec-00000000-0000-4000-8000-000000000042";
  assert.deepEqual(parseDetailLink(`mekugi:run/${itemId}:3-5`), {
    kind: "run",
    itemId,
    range: { first: 3, last: 5 },
  });
  assert.equal(parseDetailLink(`mekugi:run/${itemId}#L0`), undefined);
  assert.deepEqual(detailRange("first\r\nsecond\r\nthird\nfourth", { first: 2, last: 3 }), {
    before: "first\r\n",
    marked: "second\r\nthird",
    after: "\nfourth",
  });
  assert.equal(detailRange("a\nb\n", { first: 1, last: 3 }), undefined);
  const clipped = "first\ncut-off\n... 300 bytes omitted ...\nlast";
  assert.equal(detailRange(clipped, { first: 2, last: 2 }, true), undefined);
  assert.equal(detailRange(clipped, { first: 1, last: 1 }, true)?.marked, "first");
});

test("untrusted links and images never execute or fetch automatically", () => {
  const html = block(
    "<script>alert(1)</script> [unsafe](javascript:alert(1)) ![Remote](https://example.com/image.png) [fragment](#heading)",
  );
  assert.doesNotMatch(html, /<script>|<img|href="javascript:|data-file-link="#heading"/);
  assert.match(html, /&lt;script&gt;/);
  assert.match(html, /Image: Remote<\/a>/);
});

test("reply sections retain original-document reference links", () => {
  const summary = "Done.\n\n[evidence]: https://example.com/evidence";
  const details = "See [evidence] for the result.";
  const context = referenceContext(`## Summary\n${summary}\n## Details\n${details}`);
  const html = block(details, context);
  assert.match(html, /href="https:\/\/example.com\/evidence"[^>]*>evidence<\/a>/);
  assert.doesNotMatch(html, /Done|<h2>/);
  assert.doesNotMatch(block(details), /<a /);
});

test("question and preview Markdown renders inline code, fences, and line breaks safely", () => {
  assert.equal(
    inline("Render `Prompt.svelte` with **Markdown**"),
    "Render <code>Prompt.svelte</code> with <strong>Markdown</strong>",
  );
  const preview = block(
    "Line 1 with `inline`\nLine 2\n\n```ts\nconst x = 1;\n```\n\n<b>literal</b>",
    undefined,
    true,
  );
  assert.match(preview, /Line 1 with <code>inline<\/code><br>\nLine 2/);
  assert.match(preview, /<pre><code class="language-ts">const x = 1;\n<\/code><\/pre>/);
  assert.match(preview, /&lt;b&gt;literal&lt;\/b&gt;/);
  assert.doesNotMatch(preview, /<b>literal<\/b>/);
});
