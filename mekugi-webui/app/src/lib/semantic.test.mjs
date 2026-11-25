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
const { Stream } = await import("./stream.svelte.ts");

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
