import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { registerHooks, stripTypeScriptTypes } from "node:module";
import { test } from "node:test";
import { compile, compileModule } from "svelte/compiler";
import { render } from "svelte/server";

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

// The agent view follows the page address from the moment it loads.
globalThis.location ??= new URL("http://localhost/");
globalThis.addEventListener ??= () => {};
const { default: EntryView } = await import("./EntryView.svelte");
const { default: Transcript } = await import("./Transcript.svelte");
const { editEntryPaths, isEditEntry, isView, responseText } = await import("./kinds.ts");
const { preferences } = await import("./preferences.svelte.ts");
const { buildFeed, hookRunToEntry, itemToEntry, stream } = await import("./stream.svelte.ts");
const { default: ValueRow } = await import("./ValueRow.svelte");

const view = (values, kind = "tool") =>
  render(EntryView, {
    props: { entry: { seq: 1, kind, values }, at: "", stamp: "", owner: "/root" },
  }).body;
const edit = (path, sources) => ({ kind: "op", verb: "Edit", path, sources, effect: true });
const tags = (html, name) => html.split(`>${name}<`).length - 1;
const text = (html) => html.replace(/<!--[^]*?-->/g, "").replace(/<[^>]+>/g, "");

test("shared transcript values preserve repeated identities, reads and attachment members", () => {
  const entries = [
    {
      kind: "user",
      turn: 1,
      text: "User request",
      values: [{ kind: "text", body: "User request" }],
    },
    { kind: "text", text: "Same answer", values: [{ kind: "text", body: "Same answer" }] },
    { kind: "text", text: "Same answer", values: [{ kind: "text", body: "Same answer" }] },
    {
      kind: "tool",
      text: "Run\n```sh\nprintf once\n```",
      values: [{ kind: "op", verb: "Run", code: "printf once", lang: "sh" }],
    },
    { kind: "tool", values: [{ kind: "reads", verb: "Read", reads: [{ path: "source.go" }] }] },
    {
      kind: "attachments",
      values: [
        {
          kind: "attachments",
          verb: "Attach",
          label: "2 files, 1 skill",
          params: [
            { kind: "op", verb: "Attached", path: "attached.go", tail: ["retained file content"] },
            {
              kind: "op",
              verb: "Attached skill",
              label: "review",
              tail: ["retained skill content"],
            },
            {
              kind: "op",
              verb: "Attach failed",
              path: "missing.go",
              label: "not found",
              failed: true,
            },
          ],
        },
      ],
    },
  ];
  const html = entries
    .map(
      (entry, seq) =>
        render(EntryView, {
          props: {
            entry: { seq, agent: "/oneoff", ...entry },
            owner: "/oneoff",
            at: "",
            stamp: "",
          },
        }).body,
    )
    .join("");
  const rendered = text(html);
  assert.equal(rendered.split("User request").length - 1, 1);
  assert.equal(rendered.split("Same answer").length - 1, 2);
  assert.equal(rendered.split("printf once").length - 1, 1);
  for (const part of [
    "Read",
    "source.go",
    "attached.go",
    "review",
    "missing.go",
    "not found",
    "retained file content",
    "retained skill content",
  ])
    assert.ok(rendered.includes(part), part);
});

test("deferred JSON retains its formatted disclosure count", () => {
  const html = view([
    { kind: "op", verb: "Run", code: "{", lang: "json", fenced: true, codeLines: 4 },
  ]);
  assert.match(html, /4 lines/);
  assert.doesNotMatch(html, /1 line</);
});

test("deferred replies keep their authored summary and Details disclosure", () => {
  const html = render(EntryView, {
    props: {
      entry: {
        seq: 1,
        agent: "/root/child",
        kind: "reply",
        traffic: { kind: "message", from: "/root/child", to: "/root", incoming: true },
        autoContent: true,
        content: { attachment: "current", kind: "main", revision: 1, seq: 1 },
        values: [
          {
            kind: "message",
            from: "/root/child",
            to: "/root",
            body: "Supporting text.",
            summary: "A **compact summary**.",
          },
        ],
      },
      at: "",
      stamp: "",
      owner: "/root",
    },
  }).body;
  assert.match(html, /<strong>compact summary<\/strong>/);
  assert.match(html, /<summary[^>]*>Details<\/summary>/);
  assert.match(html, /data-slot="message"[^>]*data-ai-element="subagent"/);
  assert.match(html, /data-slot="trace-line"/);
});

test("a run of edits names its shared programs once and keeps each edit's own", () => {
  const html = view([
    { kind: "op", verb: "Run", code: "python3 - <<'PY'", lang: "bash" },
    edit("a.go", ["rm", "python3", "gofmt"]),
    edit("b.go", ["python3", "gofmt"]),
    edit("c.go", ["python3", "gofmt", "rm"]),
  ]);
  assert.match(html, />Via</);
  assert.match(html, /for all 3 files/);
  assert.equal(tags(html, "python3"), 1);
  assert.equal(tags(html, "gofmt"), 1);
  assert.equal(tags(html, "rm"), 2);
});

test("confirmed writer effects precede later pending, running and failed tests", () => {
  for (const state of [{ pending: true }, { running: true }, { exitCode: 2 }]) {
    const rendered = text(
      view([
        { kind: "op", verb: "Run", code: "python3 edit.py", lang: "bash" },
        { kind: "op", verb: "Run", code: "gofmt -w a.go", lang: "bash" },
        { ...edit("a.go", ["python3", "gofmt"]), label: "+13 -3" },
        { kind: "op", verb: "Run", code: "make test", lang: "bash", ...state },
      ]),
    );
    const positions = ["python3 edit.py", "gofmt -w a.go", "+13 −3", "make test"].map((part) =>
      rendered.indexOf(part),
    );
    assert.ok(
      positions.every((position) => position >= 0),
      rendered,
    );
    assert.deepEqual(
      positions.toSorted((a, b) => a - b),
      positions,
      rendered,
    );
  }
});

test("captured edits name their program once across the fold, apart from commands", () => {
  const creates = Array.from({ length: 12 }, (_, i) => ({
    ...edit(`/tmp/tree/file${i}.go`, ["cp"]),
    verb: "Create",
    effect: undefined,
    hiddenEdits: i === 10 ? 2 : undefined,
  }));
  const html = view([
    { kind: "op", verb: "Run", code: "cp -r src /tmp/tree", sources: ["cp"] },
    ...creates,
  ]);
  assert.match(html, /for all 12 files/);
  assert.match(html, /2 more files/);
  assert.equal(tags(html, "cp"), 2);
  assert.doesNotMatch(html, /ml-3 border-l/);
});

test("edits without a shared program keep their own", () => {
  const html = view([edit("a.go", ["rm"]), edit("b.go", ["gofmt"])]);
  assert.doesNotMatch(html, />Via</);
  assert.equal(tags(html, "rm"), 1);
  assert.equal(tags(html, "gofmt"), 1);
});

test("a wait reads as a collaboration line naming its targets", () => {
  stream.roster = {
    agents: [
      {
        name: "/root/detail-tests",
        label: "detail-tests",
        aliases: ["/root/detail-tests"],
        state: "working",
      },
      { name: "/root/review", label: "review", aliases: ["/root/review"], state: "done" },
    ],
  };
  const html = view(
    [
      {
        kind: "progress",
        body: "Waiting for",
        waits: [{ name: "/root/detail-tests" }, { name: "/root/review", status: "completed" }],
        waitTimeout: "10m0s",
        live: true,
      },
    ],
    "progress",
  );
  assert.match(html, /animate-blink/);
  assert.match(html, />detail-tests</);
  assert.match(html, />review</);
  assert.match(html, /: completed/);
  assert.match(html, /\(timeout 10m0s\)/);
  assert.doesNotMatch(html, /rounded-card border/);
  assert.doesNotMatch(html, /\/root\//);
});

test("traffic paints the published local recipient and start target", () => {
  stream.main = {
    order: [9],
    entries: {
      9: {
        seq: 9,
        identities: [{ name: "/root/worker", label: "worker", aliases: ["Main", "/root/worker"] }],
      },
    },
  };
  const html = render(EntryView, {
    props: {
      entry: {
        seq: 9,
        kind: "reply",
        agent: "/root/sender",
        traffic: { kind: "message", from: "/root/sender", to: "/root/worker", incoming: true },
        values: [{ kind: "message", from: "/root/sender", to: "Main", body: "Incoming message" }],
      },
      at: "",
      stamp: "",
      owner: "/root/worker",
    },
  }).body;
  assert.match(html, />worker</);
  assert.doesNotMatch(html, />Main<|>\/root</);
  const start = render(EntryView, {
    props: {
      entry: {
        seq: 9,
        kind: "start",
        assignment: true,
        agent: "/root/worker",
        traffic: { kind: "start", to: "/root/worker", detail: "started" },
        values: [{ kind: "start" }],
      },
      at: "",
      stamp: "",
      owner: "/root",
    },
  }).body;
  assert.match(start, />worker</);
  assert.match(start, /started/);
  assert.doesNotMatch(start, />main<|>Main<|→/);
  stream.main = { order: [], entries: {} };
});

test("delivered final answers retain both published endpoints", () => {
  stream.main = {
    order: [10],
    entries: {
      10: {
        seq: 10,
        identities: [
          { name: "/root/reviewer", label: "reviewer", aliases: ["/root/reviewer"] },
          { name: "/root/worker", label: "worker", aliases: ["/root/worker"] },
        ],
      },
    },
  };
  const html = render(EntryView, {
    props: {
      entry: {
        seq: 10,
        kind: "final",
        agent: "/root/reviewer",
        traffic: {
          kind: "final",
          from: "/root/reviewer",
          to: "/root/worker",
          detail: "finished",
          incoming: true,
        },
        values: [{ kind: "final", from: "/root/reviewer", to: "/root/worker", body: "Reviewed." }],
      },
      at: "",
      stamp: "",
      owner: "/root/worker",
    },
  }).body;
  assert.match(html, />reviewer<.*→.*>worker</s);
  stream.main = { order: [], entries: {} };
});

test("model listing and inspection render as agent progress rows without raw MCP", () => {
  for (const body of ["Listed 2 models", "Listed 1 model", "Inspected /root/worker"]) {
    const html = view([{ kind: "progress", from: "/root", body }], "progress");
    assert.match(html, /data-ai-element="agent-progress"/);
    assert.ok(html.includes(body));
    assert.doesNotMatch(html, /content|isError/);
  }
});

test("complete one-line output stays visible without a disclosure", () => {
  const value = { kind: "op", verb: "Run", code: "printf ok", tail: ["ok"] };
  const html = render(ValueRow, { props: { value } }).body;
  assert.match(html, /<pre[^>]*>.*ok/s);
  assert.doesNotMatch(html, /aria-label="Output:/);
  for (const extra of [{ tail: ["first", "last"] }, { tailOmitted: 2 }]) {
    const folded = render(ValueRow, { props: { value: { ...value, ...extra } } }).body;
    assert.match(folded, /aria-label="Output:/);
    assert.match(folded, /data-slot="trace-collapsible-trigger"/);
    assert.match(folded, /aria-expanded="false"/);
    assert.doesNotMatch(folded, /<pre/);
  }
});

test("an inline program heads its row, followed by its interpreter", () => {
  const value = { kind: "op", verb: "Run", label: "· via `python3`", lang: "python", fenced: true };
  const oneLine = render(ValueRow, { props: { value: { ...value, code: "print(1)" } } }).body;
  assert.match(text(oneLine), /print\(1\) · via python3/);
  assert.doesNotMatch(oneLine, /<pre/);

  const program = render(ValueRow, {
    props: { value: { ...value, code: "import sys\nprint(1)\n" } },
  }).body;
  assert.match(text(program), /import sys … · via python3/);
  assert.match(program, /aria-label="Script: 2 lines"/);
  assert.doesNotMatch(program, /<pre/);

  // A long one-line program shows an excerpt, with its source behind the disclosure.
  const source = `print(${"1 + ".repeat(40)}1)`;
  const long = render(ValueRow, { props: { value: { ...value, code: source } } }).body;
  assert.ok(text(long).includes(`${source.slice(0, 80).trimEnd()} … · via python3`), text(long));
  assert.match(long, /aria-label="Script: 1 line"/);
  assert.doesNotMatch(long, /<pre/);
});

test("unreached commands render Skipped while retaining their command", () => {
  const value = { kind: "op", verb: "Run", code: "printf unreachable", lang: "bash" };
  const skipped = render(ValueRow, { props: { value: { ...value, skipped: true } } }).body;
  assert.match(skipped, />Skipped</);
  assert.doesNotMatch(skipped, />Run</);
  assert.match(skipped, /printf unreachable/);

  const completed = render(ValueRow, { props: { value } }).body;
  assert.match(completed, />Run</);
  assert.doesNotMatch(completed, />Skipped</);

  const pending = render(ValueRow, { props: { value: { ...value, pending: true } } }).body;
  assert.match(pending, />pending</);
  assert.doesNotMatch(pending, />Skipped</);
});

test("user messages compose Message with right alignment and retain marked content", () => {
  const html = render(EntryView, {
    props: {
      entry: { seq: 1, agent: "You", kind: "text", turn: 2, text: "A **message**." },
      at: "12:34",
      stamp: "Time",
      turn: 2,
      owner: "/root",
    },
  }).body;
  assert.match(html, /data-slot="message" data-align="end"/);
  assert.match(html, /data-slot="message-header"/);
  assert.match(html, /<strong>message<\/strong>/);
  assert.match(html, /Turn 2/);
  assert.match(html, /12:34/);
});

test("reasoning composes Trace disclosure without a control for empty thoughts", () => {
  const html = view([{ label: "A thought", body: "Reasoning body.", live: true }], "reasoning");
  assert.match(html, /data-ai-element="reasoning"/);
  assert.match(html, /data-slot="trace-collapsible-trigger"/);
  assert.match(html, /aria-expanded="false"/);
  assert.match(html, /data-slot="trace-collapsible-content"[^>]*hidden/);
  assert.match(html, /Reasoning body/);
  const empty = view([{ label: "A thought", body: "**A thought**" }], "reasoning");
  assert.match(empty, /data-slot="trace-line"/);
  assert.doesNotMatch(empty, /data-slot="trace-collapsible-trigger"/);
});

test("native stream projections map reasoning, hooks, edits, imageView, and assistant replies", () => {
  const liveReasoning = itemToEntry(
    {
      id: "r1",
      type: "reasoning",
      summary: ["**Plan**\nFirst step", "**Verify**\nSecond step"],
      content: [],
    },
    1,
    1,
    "t1",
    "/root",
    true,
  );
  assert.equal(liveReasoning.kind, "reasoning");
  assert.equal(liveReasoning.values?.length, 2);
  assert.equal(liveReasoning.values?.[0]?.live, false);
  assert.equal(liveReasoning.values?.[1]?.live, true);
  assert.equal(liveReasoning.values?.[1]?.label, "Verify");

  const failedEdit = itemToEntry(
    {
      id: "fc-fail",
      type: "fileChange",
      status: "failed",
      changes: [{ path: "/workspace/b.go", kind: { type: "update" }, diff: "" }],
    },
    2,
    1,
    "t1",
  );
  assert.equal(isEditEntry(failedEdit), false);

  const okEdit = itemToEntry(
    {
      id: "fc-ok",
      type: "fileChange",
      status: "completed",
      changes: [
        { path: "/workspace/a.ts", kind: { type: "update" }, diff: "" },
        { path: "/workspace/z.ts", kind: { type: "update" }, diff: "" },
      ],
    },
    3,
    1,
    "t1",
  );
  assert.equal(isEditEntry(okEdit), true);
  assert.deepEqual(editEntryPaths(okEdit), ["/workspace/a.ts", "/workspace/z.ts"]);

  const img = itemToEntry(
    { id: "img1", type: "imageView", path: "/workspace/shot.png" },
    4,
    1,
    "t1",
  );
  assert.equal(isView(img), true);

  const reply = itemToEntry(
    { id: "msg1", type: "agentMessage", text: "Hello **world**", phase: null },
    5,
    1,
    "t1",
  );
  assert.equal(responseText(reply.kind, reply.text), "Hello **world**");

  const failedHook = hookRunToEntry(
    {
      id: "h1",
      eventName: "PreToolUse",
      handlerType: "command",
      executionMode: "sync",
      scope: "project",
      sourcePath: "/workspace/.mekugi/hooks/check.sh",
      source: "project",
      displayOrder: 0n,
      status: "failed",
      statusMessage: "lint failed",
      startedAt: 1700000000000n,
      completedAt: 1700000000500n,
      durationMs: 500n,
      entries: [{ kind: "stderr", text: "error on line 1" }],
    },
    6,
    "t1",
  );
  assert.equal(failedHook.kind, "hook");
  assert.equal(failedHook.outcome, "error");
  assert.equal(failedHook.values?.[0]?.hook?.hasError, true);
  assert.deepEqual(failedHook.values?.[0]?.tail, ["error on line 1"]);

  const feed = buildFeed(
    [
      {
        id: "t1",
        rootTurnId: null,
        items: [],
        itemsView: "full",
        status: "completed",
        error: null,
        startedAt: null,
        completedAt: null,
        durationMs: null,
      },
    ],
    "thread-1",
    new Set(),
    [
      {
        key: "h1:1",
        id: "h1",
        turnId: "t1",
        afterItemCount: 0,
        run: {
          id: "h1",
          eventName: "PreToolUse",
          handlerType: "command",
          executionMode: "sync",
          scope: "project",
          sourcePath: "/workspace/.mekugi/hooks/check.sh",
          source: "project",
          displayOrder: 0n,
          status: "failed",
          statusMessage: "first failed",
          startedAt: 1700000000000n,
          completedAt: 1700000000100n,
          durationMs: 100n,
          entries: [],
        },
      },
      {
        key: "h1:2",
        id: "h1",
        turnId: "t1",
        afterItemCount: 0,
        run: {
          id: "h1",
          eventName: "PreToolUse",
          handlerType: "command",
          executionMode: "sync",
          scope: "project",
          sourcePath: "/workspace/.mekugi/hooks/check.sh",
          source: "project",
          displayOrder: 0n,
          status: "completed",
          statusMessage: null,
          startedAt: 1700000001000n,
          completedAt: 1700000001100n,
          durationMs: 100n,
          entries: [],
        },
      },
    ],
  );
  assert.equal(feed.order.length, 2);
  assert.equal(feed.entries[1]?.kind, "hook");
  assert.equal(feed.entries[1]?.outcome, "error");
  assert.equal(feed.entries[2]?.kind, "hook");
  assert.equal(feed.entries[2]?.outcome, undefined);
});

test("Transcript respects Thoughts modes and filters hidden completed thought sections and timestamps", () => {
  const prevThoughts = preferences.thoughts;
  const prevHooks = preferences.hooks;
  try {
    const completedThought = itemToEntry(
      { id: "r-done", type: "reasoning", summary: ["**Old**\nDone reasoning"], content: [] },
      1,
      1,
      "t1",
      "/root",
      false,
    );
    completedThought.observed = Date.UTC(2026, 0, 1, 10, 0, 0);

    const mixedThought = itemToEntry(
      {
        id: "r-live",
        type: "reasoning",
        summary: ["**Earlier**\nCompleted section", "**Now**\nLive section"],
        content: [],
      },
      2,
      1,
      "t1",
      "/root",
      true,
    );
    mixedThought.observed = Date.UTC(2026, 0, 1, 10, 5, 0);

    preferences.setThoughts("hidden");
    const hiddenHtml = render(Transcript, {
      props: { entries: [completedThought, mixedThought] },
    }).body;
    assert.doesNotMatch(hiddenHtml, /Done reasoning/);
    assert.doesNotMatch(hiddenHtml, /Completed section/);
    assert.match(hiddenHtml, /Live section/);
    assert.match(hiddenHtml, /data-entry="2"/);

    preferences.setThoughts("collapsed");
    const collapsedHtml = render(Transcript, {
      props: { entries: [completedThought] },
    }).body;
    assert.match(collapsedHtml, /Done reasoning/);
    assert.match(collapsedHtml, /aria-expanded="false"/);

    preferences.setThoughts("expanded");
    const expandedHtml = render(Transcript, {
      props: { entries: [completedThought] },
    }).body;
    assert.match(expandedHtml, /Done reasoning/);
    assert.match(expandedHtml, /aria-expanded="true"/);
  } finally {
    preferences.setThoughts(prevThoughts);
    preferences.setHooks(prevHooks);
  }
});
