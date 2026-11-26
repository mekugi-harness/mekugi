import assert from "node:assert/strict";
import { test } from "node:test";
import { block, referenceContext } from "./markdown.ts";

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
  assert.match(html, /<span data-file-link="\/workspace\/a b.go#L12-L14"/);
  assert.match(html, /data-file-link="src\/a b.go#L12"/);
  assert.match(html, /data-file-link="file:\/\/\/workspace\/source.go#L12"/);
  assert.match(html, /data-file-link="\/workspace\/source.go:12"/);
  assert.doesNotMatch(html, /href="#"/);
  assert.doesNotThrow(() => block("[Literal](a%ZZ.md)"));
});

test("run Markdown links retain the native detail target without browser navigation", () => {
  const run = "mekugi:run/exec-00000000-0000-4000-8000-000000000042";
  const html = block(
    `[Run](${run}) [Line](${run}#L3) [Range](${run}#L3-L5) [Legacy](${run}:3-5) [Other](mekugi:other/call_42)`,
  );
  for (const suffix of ["", "#L3", "#L3-L5", ":3-5"])
    assert.ok(html.includes(`data-run-link="${run}${suffix}"`));
  assert.doesNotMatch(html, /href="#panel-content"/);
  assert.doesNotMatch(html, /href="mekugi:|data-file-link|data-run-link="mekugi:other/);
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
