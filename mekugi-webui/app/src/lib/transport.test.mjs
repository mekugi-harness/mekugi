import assert from "node:assert/strict";
import { registerHooks } from "node:module";
import { afterEach, test } from "node:test";

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
});

const { Connector, TransportError, endpointURL, parseSelection, selectionHash, unknownOutcome } =
  await import("./transport.ts");
const { protocolVersion, subprotocol } = await import("./protocol.ts");

const initResponse = {
  userAgent: "mekugi-app-server/0.1",
  codexHome: "/home/test/.codex",
  platformFamily: "unix",
  platformOs: "linux",
};

function deferred() {
  let resolve, reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

class FakeSocket extends EventTarget {
  static CONNECTING = 0;
  static OPEN = 1;
  static CLOSING = 2;
  static CLOSED = 3;
  static instances = [];
  static onSend = null;
  static autoOpen = true;
  static negotiatedProtocol = subprotocol;

  readyState = FakeSocket.CONNECTING;
  bufferedAmount = 0;
  sent = [];

  constructor(url, protocol) {
    super();
    this.url = String(url);
    this.requestedProtocol = protocol;
    this.protocol = FakeSocket.negotiatedProtocol;
    FakeSocket.instances.push(this);
    if (FakeSocket.autoOpen) {
      queueMicrotask(() => this.open());
    }
  }

  open() {
    if (this.readyState !== FakeSocket.CONNECTING) return;
    this.readyState = FakeSocket.OPEN;
    this.dispatchEvent(new Event("open"));
  }

  send(raw) {
    const parsed = JSON.parse(raw);
    this.sent.push(parsed);
    FakeSocket.onSend?.(this, parsed);
  }

  receive(payload) {
    this.dispatchEvent(new MessageEvent("message", { data: JSON.stringify(payload) }));
  }

  reply(request, result, includeJsonrpc = false) {
    const frame = includeJsonrpc
      ? { jsonrpc: "2.0", id: request.id, result }
      : { id: request.id, result };
    this.receive(frame);
  }

  replyError(request, error) {
    this.receive({ jsonrpc: "2.0", id: request.id, error });
  }

  close() {
    if (this.readyState === FakeSocket.CLOSED) return;
    this.readyState = FakeSocket.CLOSED;
    this.dispatchEvent(new Event("close"));
  }
}

const originalWebSocket = globalThis.WebSocket;
const originalFetch = globalThis.fetch;

afterEach(() => {
  FakeSocket.instances = [];
  FakeSocket.onSend = null;
  FakeSocket.autoOpen = true;
  FakeSocket.negotiatedProtocol = subprotocol;
  globalThis.WebSocket = originalWebSocket;
  globalThis.fetch = originalFetch;
});

test("endpointURL normalizes valid HTTP/HTTPS URLs and rejects invalid ones", () => {
  assert.equal(endpointURL("https://box.example/"), "https://box.example");
  assert.equal(endpointURL("http://127.0.0.1:7778///"), "http://127.0.0.1:7778");
  assert.throws(() => endpointURL("ws://box.example"));
  assert.throws(() => endpointURL("https://user:pass@box.example"));
  assert.throws(() => endpointURL("https://box.example/?token=abc"));
});

test("selectionHash and parseSelection round-trip machine, thread, and agent", () => {
  const hash = selectionHash({ machine: "m1", session: "t1", agent: "/root/helper" });
  assert.deepEqual(parseSelection(hash), { machine: "m1", session: "t1", agent: "/root/helper" });
  assert.deepEqual(
    parseSelection(selectionHash({ machine: "m1", session: "t1", agent: "/root" })),
    {
      machine: "m1",
      session: "t1",
      agent: undefined,
    },
  );
});

test("native handshake sends initialize with token + capabilities and follows with initialized notification", async () => {
  globalThis.WebSocket = FakeSocket;
  const connector = new Connector("https://box.example", "secret-token");
  FakeSocket.onSend = (socket, message) => {
    if (message.method === "initialize") {
      assert.equal(message.jsonrpc, "2.0");
      assert.equal(message.params.token, "secret-token");
      assert.equal(message.params.clientInfo.name, "mekugi-webui");
      assert.equal(message.params.capabilities.experimentalApi, true);
      assert.equal(message.params.capabilities.requiredHumanApproval, true);
      // Native app-server omits "jsonrpc" on responses
      socket.reply(message, initResponse, false);
    } else if (message.method === "thread/list") {
      socket.reply(message, { data: [], nextCursor: null, backwardsCursor: null }, false);
    }
  };

  const result = await connector.request("thread/list", { limit: 10 });
  assert.deepEqual(result.data, []);
  const socket = FakeSocket.instances[0];
  assert.equal(socket.requestedProtocol, subprotocol);
  assert.equal(socket.url, "wss://box.example/api/control");
  assert.deepEqual(
    socket.sent.map((m) => m.method),
    ["initialize", "initialized", "thread/list"],
  );
  assert.equal(socket.sent[1].id, undefined);
  connector.close();
});

test("server notifications and server requests dispatch and respond over the native channel", async () => {
  globalThis.WebSocket = FakeSocket;
  const connector = new Connector("https://box.example", "token-1");
  const notifications = [];
  const serverRequests = [];
  connector.onNotification((n) => notifications.push(n));
  connector.onServerRequest((r) => serverRequests.push(r));

  FakeSocket.onSend = (socket, message) => {
    if (message.method === "initialize") {
      socket.reply(message, initResponse);
    } else if (message.method === "thread/list") {
      socket.reply(message, { data: [], nextCursor: null, backwardsCursor: null });
    }
  };

  await connector.request("thread/list", {});
  const socket = FakeSocket.instances[0];

  // Native notification without jsonrpc field
  socket.receive({
    method: "thread/name/updated",
    params: { threadId: "t1", threadName: "Renamed" },
  });
  assert.equal(notifications.length, 1);
  assert.equal(notifications[0].method, "thread/name/updated");

  // Native server request (approval)
  socket.receive({
    id: 42,
    method: "item/commandExecution/requestApproval",
    params: {
      kind: "command",
      threadId: "t1",
      turnId: "turn1",
      itemId: "item1",
      startedAtMs: 1000,
      environmentId: null,
      command: "ls -la",
    },
  });
  assert.equal(serverRequests.length, 1);
  assert.equal(serverRequests[0].id, 42);

  connector.respond(42, { decision: "accept" });
  assert.deepEqual(socket.sent.at(-1), {
    jsonrpc: "2.0",
    id: 42,
    result: { decision: "accept" },
  });
  connector.close();
});

test("401 refusal surfaces required state and mutations fail with unknownOutcome on disconnect", async () => {
  globalThis.WebSocket = FakeSocket;
  const connector = new Connector("https://box.example", "bad-token");
  const states = [];
  const sawRequired = deferred();
  FakeSocket.onSend = (socket, message) => {
    if (message.method === "initialize") {
      socket.replyError(message, {
        code: -32000,
        message: "A valid connector token is required.",
        data: { status: 401, outcome: "not_admitted" },
      });
    }
  };
  const stop = connector.connect((state) => {
    states.push(state);
    if (state === "required") sawRequired.resolve();
  });
  await sawRequired.promise;
  assert.ok(states.includes("required"));
  stop();
  connector.close();

  // Disconnect during in-flight mutation produces unknownOutcome
  const connector2 = new Connector("https://box.example", "good-token");
  FakeSocket.onSend = (socket, message) => {
    if (message.method === "initialize") socket.reply(message, initResponse);
    else if (message.method === "turn/start") queueMicrotask(() => socket.close());
  };
  await assert.rejects(
    connector2.request("turn/start", {
      threadId: "t1",
      input: [{ type: "text", text: "hi", text_elements: [] }],
    }),
    (err) =>
      err instanceof TransportError && err.kind === "unknown" && err.message === unknownOutcome,
  );
  connector2.close();
});

test("attachment-bearing turn/start uses a separate bulk channel", async () => {
  globalThis.WebSocket = FakeSocket;
  const connector = new Connector("https://box.example", "token-1");
  FakeSocket.onSend = (socket, message) => {
    if (message.method === "initialize") socket.reply(message, initResponse);
    else if (message.id !== undefined) socket.reply(message, { ok: true });
  };
  await connector.request("thread/list", {});
  await connector.request("turn/start", {
    threadId: "t1",
    input: [{ type: "image", url: "data:image/png;base64,AAAA" }],
  });
  assert.equal(FakeSocket.instances.length, 2);
  assert.equal(FakeSocket.instances[1].sent.at(-1).method, "turn/start");
  connector.close();
});

test("HTTP resources send Mekugi-Protocol header and bearer token for version, content, and image", async () => {
  const calls = [];
  globalThis.fetch = async (url, options) => {
    calls.push({ url: String(url), options });
    if (String(url).endsWith("/api/version")) {
      return new Response(JSON.stringify({ ready: true, version: protocolVersion }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      });
    }
    if (String(url).endsWith("/api/resources/content")) {
      return new Response(
        JSON.stringify({
          threadId: "t1",
          turnId: "turn1",
          item: { id: "i1", type: "plan", text: "ok" },
        }),
        {
          status: 200,
          headers: { "Content-Type": "application/json" },
        },
      );
    }
    return new Response(new Uint8Array([1, 2, 3]), {
      status: 200,
      headers: { "Content-Type": "image/png" },
    });
  };

  const connector = new Connector("https://box.example", "tok");
  const ver = await connector.version();
  assert.equal(ver.ready, true);
  assert.equal(ver.version, protocolVersion);

  const content = await connector.resource("content", {
    thread: "t1",
    body: { thread: "t1", turnId: "turn1", itemId: "i1" },
  });
  assert.equal(content.item.id, "i1");

  const blob = await connector.resource("image", {
    thread: "t1",
    body: { itemId: "i1", index: 0, image: "abc" },
  });
  assert.equal(blob.size, 3);

  for (const call of calls) {
    assert.equal(call.options.headers["Mekugi-Protocol"], protocolVersion);
    assert.equal(call.options.headers.Authorization, "Bearer tok");
  }
  connector.close();
});
