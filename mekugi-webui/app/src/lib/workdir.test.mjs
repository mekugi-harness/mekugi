import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { registerHooks, stripTypeScriptTypes } from "node:module";
import { test } from "node:test";
import { compile, compileModule } from "svelte/compiler";
import { render } from "svelte/server";
import {
  editWithin,
  normalizeWorkdir,
  relativeEditLabel,
  segmentWorkdirRows,
  workdirGroups,
} from "./workdir.ts";

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (specifier.startsWith("$lib/"))
      specifier = new URL(specifier.slice(5), import.meta.url).href;
    if (specifier.startsWith(".") || specifier.startsWith("file:")) {
      const url = new URL(specifier, context.parentURL);
      if (url.pathname.endsWith(".js") && !existsSync(url)) {
        const source = new URL(url.href.replace(/\.js$/, ".ts"));
        if (existsSync(source)) specifier = source.href;
      } else if (!/\.(?:ts|mjs|js)$/.test(url.pathname)) {
        const index = new URL(`${url.href}/index.ts`);
        const source = new URL(`${url.href}.ts`);
        if (existsSync(index)) specifier = index.href;
        else if (existsSync(source)) specifier = source.href;
      }
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url.endsWith(".svelte") || /\.svelte\.(?:ts|js)$/.test(url)) {
      const source = readFileSync(new URL(url), "utf8");
      const options = { filename: new URL(url).pathname, generate: "server" };
      const result = url.endsWith(".svelte")
        ? compile(source, options)
        : compileModule(url.endsWith(".ts") ? stripTypeScriptTypes(source) : source, options);
      return { format: "module", source: result.js.code, shortCircuit: true };
    }
    return nextLoad(url, context);
  },
});

globalThis.location ??= new URL("http://localhost/");
globalThis.addEventListener ??= () => {};
const { default: Transcript } = await import("./Transcript.svelte");
const { default: ValueRow } = await import("./ValueRow.svelte");

const text = (html) => html.replace(/<!--[^]*?-->/g, "").replace(/<[^>]+>/g, "");

const row = (seq, values, kind = "tool", extra = {}) => ({
  entry: { seq, agent: "/root", kind, values, ...extra },
});

const reasoningRow = (seq, label = "Checking") => ({
  entry: { seq, agent: "/root", kind: "reasoning", values: [{ label, body: "details" }] },
  thoughts: [{ seq, agent: "/root", kind: "reasoning", values: [{ label, body: "details" }] }],
});

test("normalizeWorkdir strips the thread workspace and treats default dirs as empty", () => {
  assert.equal(normalizeWorkdir(undefined, "/repo"), "");
  assert.equal(normalizeWorkdir("", "/repo"), "");
  assert.equal(normalizeWorkdir(".", "/repo"), "");
  assert.equal(normalizeWorkdir("/repo", "/repo"), "");
  assert.equal(normalizeWorkdir("/repo/", "/repo"), "");
  assert.equal(normalizeWorkdir("/repo/.worktrees/topic/", "/repo"), ".worktrees/topic");
  assert.equal(normalizeWorkdir(".worktrees/topic/", "/repo"), ".worktrees/topic");
  assert.equal(normalizeWorkdir("/repo-other/sub", "/repo"), "/repo-other/sub");
  assert.equal(normalizeWorkdir("/tmp/other", "/repo"), "/tmp/other");
});

test("editWithin and relativeEditLabel shorten paths strictly inside the group workdir", () => {
  const dir = ".worktrees/topic";
  assert.equal(
    editWithin({ kind: "op", verb: "Edit", path: ".worktrees/topic/internal/edit.go" }, dir),
    "internal/edit.go",
  );
  assert.equal(
    editWithin(
      { kind: "op", verb: "Edit", path: "/repo/.worktrees/topic/internal/edit.go" },
      dir,
      "/repo",
    ),
    "internal/edit.go",
  );
  assert.equal(editWithin({ kind: "op", verb: "Edit", path: ".worktrees/other/edit.go" }, dir), "");
  assert.equal(
    editWithin({ kind: "op", verb: "Edit", path: ".worktrees/topic/../escape.go" }, dir),
    "",
  );
  assert.equal(editWithin({ kind: "op", verb: "Edit", path: ".worktrees/topic/." }, dir), "");
  assert.equal(
    editWithin({ kind: "op", verb: "Run", path: ".worktrees/topic/internal/edit.go" }, dir),
    "",
  );

  // Move requires both endpoints to lie inside the directory.
  assert.equal(
    editWithin(
      { kind: "op", verb: "Move", path: ".worktrees/topic/a.go => .worktrees/topic/b.go" },
      dir,
    ),
    "a.go => b.go",
  );
  assert.equal(
    editWithin({ kind: "op", verb: "Move", path: ".worktrees/topic/a.go => other/b.go" }, dir),
    "",
  );
  assert.equal(
    editWithin({ kind: "op", verb: "Move", path: ".worktrees/topic/src/{old=>new}.go" }, dir),
    "src/{old=>new}.go",
  );

  // Label-only edit values rewrite only their leading code span.
  const labelEdit = {
    kind: "op",
    verb: "Edit",
    label: "`.worktrees/topic/internal/edit.go` +1 -1",
  };
  assert.equal(editWithin(labelEdit, dir), "internal/edit.go");
  assert.equal(relativeEditLabel(labelEdit, dir), "`internal/edit.go` +1 -1");
});

test("workdirGroups mirrors 0ef4e99/0f29ba4 grouping, edit adoption, and reasoning bridging", () => {
  const rows = [
    row(1, [
      { kind: "op", verb: "Edit", path: ".worktrees/topic/internal/edit.go", label: "+1 -1" },
    ]),
    row(2, [
      { kind: "op", verb: "Run", code: "go test ./...", workdir: ".worktrees/topic" },
      { kind: "op", verb: "Run", batchExit: true, exitCode: 1, tail: ["test failed"] },
    ]),
    row(3, [
      { kind: "op", verb: "Search", label: "`package` in `internal`", workdir: ".worktrees/topic" },
    ]),
    row(4, [{ kind: "op", verb: "Run", code: "go version" }]),
    row(5, [{ kind: "op", verb: "Run", code: "go env GOROOT", workdir: ".worktrees/topic" }]),
    reasoningRow(6, "Hidden thought"),
    row(7, [{ kind: "op", verb: "Run", code: "go env GOPATH", workdir: ".worktrees/topic" }]),
    row(8, [{ kind: "text", body: "Visible answer." }], "text", { text: "Visible answer." }),
    row(9, [{ kind: "op", verb: "Run", code: "go env GOOS", workdir: "sub" }]),
  ];

  const groups = workdirGroups(rows, "/repo");
  assert.deepEqual(groups, [
    ".worktrees/topic",
    ".worktrees/topic",
    ".worktrees/topic",
    "",
    ".worktrees/topic",
    ".worktrees/topic",
    ".worktrees/topic",
    "",
    "",
  ]);

  const segments = segmentWorkdirRows(rows, groups);
  assert.deepEqual(
    segments.map((segment) =>
      segment.kind === "group"
        ? { kind: "group", workdir: segment.workdir, seqs: segment.rows.map((r) => r.entry.seq) }
        : { kind: "row", seq: segment.row.entry.seq },
    ),
    [
      { kind: "group", workdir: ".worktrees/topic", seqs: [1, 2, 3] },
      { kind: "row", seq: 4 },
      { kind: "group", workdir: ".worktrees/topic", seqs: [5, 6, 7] },
      { kind: "row", seq: 8 },
      { kind: "row", seq: 9 },
    ],
  );
});

test("workdirGroups refuses multi-file edits with any path outside the group and trailing reasoning", () => {
  const rows = [
    row(1, [
      { kind: "op", verb: "Edit", path: ".worktrees/topic/a.go" },
      { kind: "op", verb: "Edit", path: "outside/b.go" },
    ]),
    row(2, [{ kind: "op", verb: "Run", code: "go test", workdir: "/repo/.worktrees/topic" }]),
    row(3, [{ kind: "op", verb: "Run", code: "go vet", workdir: "/repo/.worktrees/topic" }]),
    reasoningRow(4, "Trailing thought"),
    row(5, [{ kind: "op", verb: "Run", code: "git status", workdir: "/repo" }]),
  ];

  assert.deepEqual(workdirGroups(rows, "/repo"), [
    "",
    ".worktrees/topic",
    ".worktrees/topic",
    "",
    "",
  ]);
});

test("Transcript renders sticky workdir headers, suppresses redundant inline workdir, and relativizes edit paths", () => {
  const entries = [
    {
      seq: 1,
      agent: "/root",
      kind: "tool",
      values: [
        { kind: "op", verb: "Edit", path: ".worktrees/topic/internal/edit.go", label: "+1 -1" },
      ],
    },
    {
      seq: 2,
      agent: "/root",
      kind: "tool",
      values: [{ kind: "op", verb: "Run", code: "go test ./...", workdir: ".worktrees/topic" }],
    },
    {
      seq: 3,
      agent: "/root",
      kind: "reasoning",
      values: [{ label: "Checking", body: "Inspecting output" }],
    },
    {
      seq: 4,
      agent: "/root",
      kind: "tool",
      values: [{ kind: "op", verb: "Run", code: "go vet ./...", workdir: ".worktrees/topic" }],
    },
    {
      seq: 5,
      agent: "/root",
      kind: "text",
      text: "Done.",
    },
    {
      seq: 6,
      agent: "/root",
      kind: "tool",
      values: [{ kind: "op", verb: "Run", code: "go env GOOS", workdir: "sub" }],
    },
  ];

  const html = render(Transcript, {
    props: {
      entries,
      owner: "/root",
      workspace: "/repo",
      start: 0,
      span: 0,
      jumps: 0,
      empty: "Empty",
    },
  }).body;

  assert.match(html, /data-workdir-group="\.worktrees\/topic"/);
  assert.match(html, /data-workdir-heading="\.worktrees\/topic"[^>]*class="[^"]*sticky top-0/);
  // Relative edit path displayed, while full path stays in title for hover/inspection.
  assert.match(
    html,
    /title="\.worktrees\/topic\/internal\/edit\.go"[^>]*><bdi[^>]*>internal\/edit\.go<\/bdi>/,
  );
  // Redundant inline "in .worktrees/topic" is suppressed inside the group.
  const rendered = text(html);
  assert.ok(!rendered.includes("in .worktrees/topic"), rendered);
  // Single isolated non-default workdir row keeps its inline suffix and gets no group header.
  assert.doesNotMatch(html, /data-workdir-group="sub"/);
  assert.ok(rendered.includes("in sub"), rendered);
});

test("ValueRow keeps full path in title while displaying relative path under groupWorkdir", () => {
  const value = {
    kind: "op",
    verb: "Edit",
    path: "codex-rs/core/src/lib.rs",
    label: "+3 -1",
    workdir: "codex-rs",
  };
  const grouped = render(ValueRow, {
    props: { value, groupWorkdir: "codex-rs", workspace: "/repo" },
  }).body;
  assert.match(
    grouped,
    /title="codex-rs\/core\/src\/lib\.rs"[^>]*><bdi[^>]*>core\/src\/lib\.rs<\/bdi>/,
  );
  assert.ok(!text(grouped).includes("in codex-rs"), text(grouped));
  assert.equal(value.path, "codex-rs/core/src/lib.rs");

  const standalone = render(ValueRow, { props: { value } }).body;
  assert.match(
    standalone,
    /title="codex-rs\/core\/src\/lib\.rs"[^>]*><bdi[^>]*>codex-rs\/core\/src\/lib\.rs<\/bdi>/,
  );
  assert.ok(text(standalone).includes("in codex-rs"), text(standalone));
});

test("compound commands with workdir only on the first step stay grouped", () => {
  const rows = [
    row(1, [{ kind: "op", verb: "Run", code: "go test ./...", workdir: "docs" }]),
    row(2, [
      { kind: "op", verb: "Run", code: "go vet ./...", workdir: "docs" },
      { kind: "op", verb: "Run", code: "go build ./..." },
    ]),
  ];
  assert.deepEqual(workdirGroups(rows, "/repo"), ["docs", "docs"]);
});

test("bulk edits with pathless folded-edits markers are still adopted into the workdir group", () => {
  const rows = [
    row(1, [
      { kind: "op", verb: "Create", path: ".worktrees/topic/a.go" },
      { kind: "op", verb: "Create", hiddenEdits: 5 },
    ]),
    row(2, [{ kind: "op", verb: "Run", code: "go test ./...", workdir: ".worktrees/topic" }]),
  ];
  assert.deepEqual(workdirGroups(rows, "/repo"), [".worktrees/topic", ".worktrees/topic"]);
});

test("segmentWorkdirRows preserves group keys when paging prepends rows into the same group", () => {
  const keys = new Map();
  const page1 = [
    row(51, [{ kind: "op", verb: "Run", code: "cmd 51", workdir: "docs" }]),
    row(52, [{ kind: "op", verb: "Run", code: "cmd 52", workdir: "docs" }]),
  ];
  const seg1 = segmentWorkdirRows(page1, ["docs", "docs"], keys);
  assert.equal(seg1.length, 1);
  const initialKey = seg1[0].key;

  const page2 = [
    row(49, [{ kind: "op", verb: "Run", code: "cmd 49", workdir: "docs" }]),
    row(50, [{ kind: "op", verb: "Run", code: "cmd 50", workdir: "docs" }]),
    ...page1,
  ];
  const seg2 = segmentWorkdirRows(page2, ["docs", "docs", "docs", "docs"], keys);
  assert.equal(seg2.length, 1);
  assert.equal(seg2[0].key, initialKey);
});

test("ValueRow preserves already-relative git status change paths and adds tooltips to shortened label edits", () => {
  const gitStatus = {
    kind: "op",
    verb: "Status",
    workdir: "docs",
    changes: [{ code: "M", label: "docs/intro.md" }],
  };
  const statusHtml = render(ValueRow, {
    props: { value: gitStatus, groupWorkdir: "docs", workspace: "/repo" },
  }).body;
  assert.match(statusHtml, /<bdi[^>]*>docs\/intro\.md<\/bdi>/);

  const labelEdit = {
    kind: "op",
    verb: "Edit",
    label: "`.worktrees/topic/internal/edit.go` +1 -1",
  };
  const editHtml = render(ValueRow, {
    props: { value: labelEdit, groupWorkdir: ".worktrees/topic", workspace: "/repo" },
  }).body;
  assert.match(editHtml, /title="\.worktrees\/topic\/internal\/edit\.go"/);
  assert.ok(text(editHtml).includes("internal/edit.go"), text(editHtml));
});
