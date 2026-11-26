import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { registerHooks, stripTypeScriptTypes } from "node:module";
import { test } from "node:test";
import { compileModule } from "svelte/compiler";

registerHooks({
  resolve(specifier, context, nextResolve) {
    if (
      specifier.startsWith("./") &&
      context.parentURL?.includes("/app/src/lib/") &&
      !/\.(?:ts|mjs|js)$/.test(specifier)
    ) {
      specifier += ".ts";
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url.endsWith(".svelte.ts")) {
      return {
        format: "module",
        source: compileModule(stripTypeScriptTypes(readFileSync(new URL(url), "utf8")), {
          filename: new URL(url).pathname,
          generate: "client",
        }).js.code,
        shortCircuit: true,
      };
    }
    return nextLoad(url, context);
  },
});

globalThis.localStorage = {
  length: 0,
  key: () => null,
  getItem: () => null,
  setItem: () => {},
  removeItem: () => {},
};
globalThis.location = { hash: "", pathname: "/", search: "" };
globalThis.history = { pushState: () => {}, replaceState: () => {} };
globalThis.addEventListener = () => {};

const { ThreadControl, serverRequestToPrompt, toNativeUserInput } =
  await import("./semantic.svelte.ts");
const { Stream, estimateCostUsd, itemToEntry, unwrapShellCommand } =
  await import("./stream.svelte.ts");

function fakeMachine(handlers = {}) {
  const notificationListeners = new Set();
  const requestListeners = new Set();
  const openListeners = new Set();
  const requests = [];
  const responses = [];
  return {
    id: "m1",
    label: "Test Box",
    connection: "open",
    requests,
    responses,
    onNotification(fn) {
      notificationListeners.add(fn);
      return () => notificationListeners.delete(fn);
    },
    onServerRequest(fn) {
      requestListeners.add(fn);
      return () => requestListeners.delete(fn);
    },
    onOpen(fn) {
      openListeners.add(fn);
      return () => openListeners.delete(fn);
    },
    async request(method, params) {
      requests.push({ method, params });
      if (handlers[method]) return handlers[method](params);
      return {};
    },
    async resource(kind, params) {
      requests.push({ kind, params });
      if (handlers[kind]) return handlers[kind](params);
      return {};
    },
    respond(id, result) {
      responses.push({ id, result });
    },
    emitNotification(n) {
      for (const fn of notificationListeners) fn(n);
    },
    emitServerRequest(r) {
      for (const fn of requestListeners) fn(r);
    },
  };
}

test("toNativeUserInput converts text and attachments to native UserInput items", () => {
  const input = toNativeUserInput("Hello", [
    { source: "path", path: "docs/diagram.png" },
    { source: "upload", name: "shot.png", data: "data:image/png;base64,AA==" },
    { source: "upload", name: "clip.png", mime: "image/png", data: "iVBORw==" },
    { source: "upload", name: "photo.jpg", data: "/9j/4A==" },
  ]);
  assert.deepEqual(input, [
    { type: "text", text: "Hello", text_elements: [] },
    { type: "localImage", path: "docs/diagram.png" },
    { type: "image", url: "data:image/png;base64,AA==" },
    { type: "image", url: "data:image/png;base64,iVBORw==" },
    { type: "image", url: "data:image/jpeg;base64,/9j/4A==" },
  ]);
});

test("queued sendInput supplies clientUserMessageId and refreshQueue reads response.data", async () => {
  const queuedItem = {
    id: "q1",
    input: [{ type: "text", text: "Run tests", text_elements: [] }],
    clientUserMessageId: "c1",
    createdAt: 1700000000,
  };
  const machine = fakeMachine({
    "thread/queue/list": () => ({ data: [queuedItem], nextCursor: null }),
    "thread/queue/add": () => ({ queuedSubmission: queuedItem }),
  });
  const control = new ThreadControl();
  control.connect("t1", machine);
  await new Promise((r) => setTimeout(r, 10));
  assert.equal(control.queued.length, 1);
  assert.equal(control.queued[0].id, "q1");

  const err = await control.sendInput("Run tests", [], { queue: true });
  assert.equal(err, undefined);
  const addReq = machine.requests.find((r) => r.method === "thread/queue/add");
  assert.ok(addReq);
  assert.equal(addReq.params.threadId, "t1");
  assert.equal(typeof addReq.params.clientUserMessageId, "string");
  assert.ok(addReq.params.clientUserMessageId.length > 0);
  control.release();
});

test("shellGuard/requestApproval preserves argument boundaries with shell quoting", () => {
  const prompt = serverRequestToPrompt(
    {
      id: 21,
      method: "shellGuard/requestApproval",
      params: {
        threadId: "t1",
        turnId: "turn1",
        startedAtMs: 1000,
        executable: "rm",
        argv: ["-rf", "/tmp/a b", "it's", ""],
        cwd: "/repo",
      },
    },
    "t1",
  );
  assert.equal(prompt?.kind, "approval");
  assert.equal(prompt.approval.subject, "rm -rf '/tmp/a b' 'it'\\''s' ''");
});

test("one-time approvals never offer session-wide grants and reply with accept/decline only", () => {
  const machine = fakeMachine({ "thread/queue/list": () => ({ data: [], nextCursor: null }) });
  const control = new ThreadControl();
  control.connect("t1", machine);

  machine.emitServerRequest({
    id: 10,
    method: "item/commandExecution/requestApproval",
    params: {
      kind: "command",
      threadId: "t1",
      turnId: "turn1",
      itemId: "item1",
      startedAtMs: 1000,
      environmentId: null,
      command: "rm -rf /tmp/foo",
      cwd: "/repo",
    },
  });

  assert.equal(control.prompts.approvals.length, 1);
  const approval = control.prompts.approvals[0];
  assert.deepEqual(
    approval.choices.map((c) => c.decision),
    ["accept", "decline"],
  );

  const err = control.decideApproval(approval, 0);
  assert.equal(err, undefined);
  assert.deepEqual(machine.responses, [{ id: 10, result: { decision: "accept" } }]);
  assert.equal(control.prompts.approvals.length, 0);
  control.release();
});

test("secret user-input prompts are blocked from remote answering and shellGuard/requestSecret is ignored", () => {
  const secretReq = serverRequestToPrompt(
    {
      id: 11,
      method: "shellGuard/requestSecret",
      params: { threadId: "t1", turnId: "turn1", startedAtMs: 1000 },
    },
    "t1",
  );
  assert.equal(secretReq, undefined);

  const machine = fakeMachine({ "thread/queue/list": () => ({ data: [], nextCursor: null }) });
  const control = new ThreadControl();
  control.connect("t1", machine);
  machine.emitServerRequest({
    id: 12,
    method: "item/tool/requestUserInput",
    params: {
      threadId: "t1",
      turnId: "turn1",
      itemId: "item1",
      isBlocking: true,
      autoResolutionMs: null,
      questions: [
        {
          id: "q1",
          header: "API key",
          question: "Enter secret key",
          isOther: true,
          isSecret: true,
          options: null,
        },
      ],
    },
  });
  assert.equal(control.prompts.questions.length, 1);
  assert.equal(control.prompts.questions[0].secretBlocked, true);
  const err = control.answerQuestions(control.prompts.questions[0], [
    { question: "q1", text: "super-secret" },
  ]);
  assert.match(err, /locally/);
  assert.equal(machine.responses.length, 0);
  control.release();
});

test("Stream hydrates via thread/resume and applies live ServerNotification deltas", async () => {
  const machine = fakeMachine({
    "thread/resume": () => ({
      thread: {
        id: "t1",
        sessionId: "s1",
        forkedFromId: null,
        parentThreadId: null,
        preview: "Hello",
        ephemeral: false,
        section: null,
        sectionEnteredAt: null,
        projectId: null,
        historyMode: "full",
        modelProvider: "openai",
        model: "gpt-5",
        reasoningEffort: "medium",
        serviceTier: null,
        contextTokens: 100,
        contextWindow: 128000,
        completedCompactions: 0,
        agentStatus: null,
        createdAt: 1700000000,
        updatedAt: 1700000000,
        recencyAt: null,
        status: { type: "idle" },
        path: null,
        cwd: "/repo",
        cliVersion: "0.1",
        originator: null,
        source: "cli",
        threadSource: null,
        agentNickname: null,
        agentRole: null,
        gitInfo: null,
        name: "My Thread",
        turns: [],
      },
      model: "gpt-5",
      modelProvider: "openai",
      serviceTier: null,
      disabledPluginIds: [],
      cwd: "/repo",
      instructionSources: [],
      approvalPolicy: "on-request",
      approvalsReviewer: "user",
      sandbox: { type: "workspaceWrite" },
      reasoningEffort: "medium",
      collaborationMode: null,
      turnsBackwardsCursor: null,
      itemsBackwardsCursor: null,
    }),
    "thread/subtree/list": () => ({ data: [], nextCursor: null, backwardsCursor: null }),
    "thread/changes/read": () => ({
      threadId: "t1",
      workspace: "/repo",
      changes: [],
      files: [],
      totalAdditions: 0,
      totalDeletions: 0,
    }),
  });

  const s = new Stream();
  s.connect("t1", machine);
  await new Promise((r) => setTimeout(r, 10));
  assert.equal(s.session?.title, "My Thread");
  assert.equal(s.session?.model, "gpt-5");

  machine.emitNotification({
    method: "turn/started",
    params: {
      threadId: "t1",
      turn: {
        id: "turn-1",
        rootTurnId: null,
        items: [],
        itemsView: "full",
        status: "inProgress",
        error: null,
        startedAt: 1700000001,
        completedAt: null,
        durationMs: null,
      },
    },
  });
  assert.equal(s.activeTurnId, "turn-1");

  machine.emitNotification({
    method: "item/started",
    params: {
      threadId: "t1",
      turnId: "turn-1",
      startedAtMs: 1700000001000,
      item: {
        type: "agentMessage",
        id: "msg-1",
        text: "Hel",
        moderation: null,
        phase: null,
        memoryCitation: null,
        delivery: null,
        questions: null,
      },
    },
  });
  machine.emitNotification({
    method: "item/agentMessage/delta",
    params: { threadId: "t1", turnId: "turn-1", itemId: "msg-1", delta: "lo!" },
  });
  assert.equal(s.main.order.length, 1);
  assert.equal(s.main.entries[1].text, "Hello!");

  machine.emitNotification({
    method: "thread/started",
    params: {
      thread: {
        id: "t-child",
        sessionId: "s2",
        forkedFromId: null,
        parentThreadId: "t1",
        preview: "Child task",
        ephemeral: true,
        section: null,
        sectionEnteredAt: null,
        projectId: null,
        historyMode: "full",
        modelProvider: "openai",
        model: "gpt-5",
        reasoningEffort: "medium",
        serviceTier: null,
        contextTokens: 0,
        contextWindow: 128000,
        completedCompactions: 0,
        agentStatus: null,
        createdAt: 1700000002,
        updatedAt: 1700000002,
        recencyAt: null,
        status: { type: "active", activeFlags: [] },
        path: null,
        cwd: "/repo",
        cliVersion: "0.1",
        originator: null,
        source: "subagent",
        threadSource: null,
        agentNickname: "worker",
        agentRole: "worker",
        gitInfo: null,
        name: "worker",
        turns: [],
      },
    },
  });
  assert.ok(s.roster.agents.some((a) => a.thread === "t-child"));

  s.close();
  assert.ok(
    machine.requests.some((r) => r.method === "thread/unsubscribe" && r.params.threadId === "t1"),
  );
});

test("unwrapShellCommand strips shell -lc/-c wrappers across quote styles", () => {
  assert.equal(unwrapShellCommand("/home/linuxbrew/.linuxbrew/bin/bash -lc pwd"), "pwd");
  assert.equal(unwrapShellCommand("/bin/bash -lc 'rm -rf /tmp/foo'"), "rm -rf /tmp/foo");
  assert.equal(unwrapShellCommand("bash -c 'echo '\\''hi'\\'''"), "echo 'hi'");
  assert.equal(unwrapShellCommand('zsh -lc "echo \\"$HOME\\""'), 'echo "$HOME"');
  assert.equal(unwrapShellCommand("git status -sb"), "git status -sb");
});

test("itemToEntry maps commandExecution with bash lang and fileChange as per-file op rows", () => {
  const cmdEntry = itemToEntry(
    {
      id: "c1",
      type: "commandExecution",
      command: "/bin/bash -lc 'git diff --stat'",
      cwd: "/repo",
      status: "completed",
      exitCode: 0,
      durationMs: 42,
      aggregatedOutput: "a.ts | 2 +-\n",
    },
    1,
    "turn-1",
    "/root",
  );
  assert.equal(cmdEntry.kind, "tool");
  assert.equal(cmdEntry.values?.[0]?.kind, "op");
  assert.equal(cmdEntry.values?.[0]?.verb, "Run");
  assert.equal(cmdEntry.values?.[0]?.lang, "bash");
  assert.equal(cmdEntry.values?.[0]?.code, "git diff --stat");

  const editEntry = itemToEntry(
    {
      id: "f1",
      type: "fileChange",
      status: "completed",
      changes: [
        { path: "a.ts", kind: { type: "add" }, diff: "+line1\n+line2\n" },
        {
          path: "b.ts",
          kind: { type: "update", move_path: "c.ts" },
          diff: "@@ -1 +1 @@\n-old\n+new\n",
        },
        { path: "d.ts", kind: { type: "delete" }, diff: "-gone\n" },
      ],
    },
    2,
    "turn-1",
    "/root",
  );
  assert.equal(editEntry.values?.length, 3);
  assert.equal(editEntry.values?.[0]?.verb, "Create");
  assert.equal(editEntry.values?.[0]?.label, "+2 −0");
  assert.equal(editEntry.values?.[0]?.lang, "diff");
  assert.equal(editEntry.values?.[1]?.verb, "Move");
  assert.equal(editEntry.values?.[1]?.path, "b.ts => c.ts");
  assert.equal(editEntry.values?.[1]?.label, "+1 −1");
  assert.equal(editEntry.values?.[2]?.verb, "Delete");
  assert.equal(editEntry.values?.[2]?.label, "+0 −1");
});

test("turn/completed with summary itemsView preserves existing turn items and tracks cost & TPS", async () => {
  const machine = fakeMachine({
    "thread/resume": () => ({
      thread: {
        id: "t-summary",
        preview: "hi",
        cwd: "/repo",
        createdAt: 1700000000,
        status: { type: "idle" },
        turns: [],
      },
      model: "gpt-6.2-sol",
      reasoningEffort: "medium",
      serviceTier: null,
    }),
    "thread/subtree/list": () => ({ data: [] }),
    "thread/changes/read": () => ({
      threadId: "t-summary",
      workspace: "/repo",
      totalAdditions: 0,
      totalDeletions: 0,
      changes: [],
      files: [],
    }),
  });
  const s = new Stream();
  s.connect("t-summary", machine);
  await new Promise((r) => setTimeout(r, 15));

  machine.emitNotification({
    method: "turn/started",
    params: {
      threadId: "t-summary",
      turn: { id: "turn-1", status: "inProgress", items: [], itemsView: "full", error: null },
    },
  });
  machine.emitNotification({
    method: "item/completed",
    params: {
      threadId: "t-summary",
      turnId: "turn-1",
      item: { id: "u1", type: "userMessage", content: [{ type: "text", text: "Run pwd" }] },
    },
  });
  machine.emitNotification({
    method: "item/completed",
    params: {
      threadId: "t-summary",
      turnId: "turn-1",
      item: {
        id: "c1",
        type: "commandExecution",
        command: "/bin/bash -lc pwd",
        cwd: "/repo",
        status: "completed",
        exitCode: 0,
        aggregatedOutput: "/repo\n",
      },
    },
  });
  await new Promise((r) => setTimeout(r, 10));
  machine.emitNotification({
    method: "thread/tokenUsage/updated",
    params: {
      threadId: "t-summary",
      tokenUsage: {
        total: {
          totalTokens: 1500,
          inputTokens: 1000,
          cachedInputTokens: 200,
          cacheWriteInputTokens: 0,
          outputTokens: 500,
          reasoningOutputTokens: 0,
        },
        last: {
          totalTokens: 1500,
          inputTokens: 1000,
          cachedInputTokens: 200,
          cacheWriteInputTokens: 0,
          outputTokens: 500,
          reasoningOutputTokens: 0,
        },
        modelContextWindow: 272000,
      },
    },
  });
  machine.emitNotification({
    method: "turn/completed",
    params: {
      threadId: "t-summary",
      turn: {
        id: "turn-1",
        status: "completed",
        itemsView: "summary",
        items: [{ id: "a1", type: "agentMessage", text: "Done." }],
        error: null,
      },
    },
  });

  assert.equal(s.main.order.length, 3);
  assert.equal(s.main.entries[s.main.order[0]].kind, "user");
  assert.equal(s.main.entries[s.main.order[1]].kind, "tool");
  assert.equal(s.main.entries[s.main.order[2]].kind, "assistant");

  const root = s.roster.agents[0];
  assert.ok(root.cost != null && root.cost > 0);
  assert.ok(root.tps != null && root.tps > 0);
  assert.ok(root.work != null && root.work > 0);
  s.close();
});

test("estimateCostUsd computes standard, fast, cached, and unknown model prices", () => {
  const std = estimateCostUsd("gpt-6.2-sol", undefined, {
    inputTokens: 200_000,
    cachedInputTokens: 100_000,
    outputTokens: 100_000,
  });
  // gpt-6.2-sol standard (<=272k): $2/1M uncached (0.2), $0.1/1M cached (0.01), $10/1M output (1.0)
  assert.ok(Math.abs(std - (0.2 + 0.01 + 1.0)) < 1e-9);
  const fast = estimateCostUsd("openai/gpt-6.2-sol", "fast", {
    inputTokens: 200_000,
    cachedInputTokens: 0,
    outputTokens: 0,
  });
  assert.ok(Math.abs(fast - 0.8) < 1e-9);
  const longCtx = estimateCostUsd("gpt-6.2-sol", undefined, {
    inputTokens: 300_000,
    cachedInputTokens: 0,
    outputTokens: 0,
  });
  // Long context (>272k): $4/1M input -> $1.20
  assert.ok(Math.abs(longCtx - 1.2) < 1e-9);
  assert.equal(
    estimateCostUsd("unknown-model", undefined, { inputTokens: 100, outputTokens: 50 }),
    undefined,
  );
});

test("ThreadControl handles slash commands locally instead of forwarding to turn/start", async () => {
  const machine = fakeMachine();
  const control = new ThreadControl();
  control.connect("t-slash", machine);

  assert.equal(await control.sendInput("/compact", []), undefined);
  assert.equal(await control.sendInput("/model gpt-6.2-sol", []), undefined);
  assert.equal(await control.sendInput("/effort high", []), undefined);
  assert.equal(await control.sendInput("/reasoning low", []), undefined);
  assert.equal(await control.sendInput("/tier fast", []), undefined);
  assert.equal(await control.sendInput("/tier standard", []), undefined);
  assert.equal(await control.sendInput("/rename New Title", []), undefined);
  assert.equal(await control.sendInput("/archive", []), undefined);
  assert.match(await control.sendInput("/model", []), /Specify a model name/);

  assert.equal(
    machine.requests.some((r) => r.method === "turn/start"),
    false,
  );
  assert.ok(machine.requests.some((r) => r.method === "thread/compact/start"));
  assert.ok(
    machine.requests.some(
      (r) => r.method === "thread/settings/update" && r.params.model === "gpt-6.2-sol",
    ),
  );
  assert.ok(
    machine.requests.some(
      (r) => r.method === "thread/settings/update" && r.params.effort === "high",
    ),
  );
  assert.ok(
    machine.requests.some(
      (r) => r.method === "thread/settings/update" && r.params.serviceTier === null,
    ),
  );
  assert.ok(
    machine.requests.some((r) => r.method === "thread/name/set" && r.params.name === "New Title"),
  );
  assert.ok(machine.requests.some((r) => r.method === "thread/archive"));
  control.release();
});
