import {
  protocolVersion,
  subprotocol,
  type ClientNotification,
  type InitializeParams,
  type InitializeResponse,
  type RemoteHumanMethod,
  type RequestId,
  type ResourceRequest,
  type ServerNotification,
  type ServerRequest,
  type TurnStartParams,
  type TurnSteerParams,
} from "./protocol";

export type { RemoteHumanMethod, ServerNotification, ServerRequest } from "./protocol";

export type Connection = "connecting" | "open" | "lost" | "required" | "incompatible";
export interface RequestOptions {
  signal?: AbortSignal;
}
export type ResourceKind = "content" | "file" | "image" | "attachment-image";
type StateListener = (state: Connection, error?: TransportError) => void;
type OpenListener = (info: InitializeResponse) => void;
type NotificationListener = (notification: ServerNotification) => void;
type ServerRequestListener = (request: ServerRequest) => void;

export interface RPCErrorPayload {
  code: number;
  message: string;
  data?: {
    status?: number;
    outcome?: string;
    reason?: string;
    threadId?: string;
  };
}

interface WireMessage {
  jsonrpc?: string;
  id?: RequestId;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: RPCErrorPayload;
}

export class TransportError extends Error {
  readonly kind: "network" | "unknown" | "required" | "incompatible" | "refused" | "canceled";
  readonly status?: number;
  readonly rpcError?: RPCErrorPayload;
  constructor(
    message: string,
    kind: TransportError["kind"],
    status?: number,
    rpcError?: RPCErrorPayload,
  ) {
    super(message);
    this.kind = kind;
    this.status = status;
    this.rpcError = rpcError;
  }
}

export const unknownOutcome =
  "The request outcome is unknown. Check this machine's session before submitting again. Nothing was retried.";
export const unreachable =
  "Cannot reach this machine's connector. Check its address and allowed UI origin.";
const backoff = (attempt: number): number => Math.min(1000 * 2 ** attempt, 15_000);
const canceled = (): TransportError =>
  new TransportError("The browser request was canceled.", "canceled");

export function errorText(error: unknown, mutation = false): string {
  return error instanceof TransportError ? error.message : mutation ? unknownOutcome : unreachable;
}

// getRandomValues remains available when the UI is served over plain HTTP.
export function newIdentity(): string {
  return crypto.getRandomValues(new Uint32Array(4)).join("-");
}

export function endpointURL(endpoint: string): string {
  let url: URL;
  try {
    url = new URL(endpoint);
  } catch {
    throw new Error("Enter an absolute HTTP or HTTPS connector URL.");
  }
  if (
    !["http:", "https:"].includes(url.protocol) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  )
    throw new Error(
      "Use an HTTP or HTTPS connector URL without credentials, query parameters or a fragment.",
    );
  return url.href.replace(/\/+$/, "");
}

export type ReadFailure = { kind: "failed"; text: string };
export function readFailure(error: unknown): ReadFailure {
  return { kind: "failed", text: errorText(error) };
}

function remoteError(error: RPCErrorPayload, initialize = false): TransportError {
  const status = error.data?.status;
  if (status === 401)
    return new TransportError("This machine requires a valid bearer token.", "required", status);
  if (initialize && status === 409)
    return new TransportError(
      "This connector uses an incompatible Mekugi protocol.",
      "incompatible",
      status,
    );
  if (error.data?.outcome === "unknown")
    return new TransportError(`${error.message} ${unknownOutcome}`, "unknown", status);
  return new TransportError(error.message, "refused", status, error);
}

const READ_ONLY_METHODS = new Set<RemoteHumanMethod>([
  "thread/revert/preview",
  "initialize",
  "model/list",
  "permissionProfile/list",
  "skills/list",
  "featureUse/read",
  "server/diagnostics",
  "thread/list",
  "thread/presets/list",
  "thread/inspect",
  "thread/subtree/list",
  "thread/changes/read",
  "thread/backgroundTerminals/list",
  "thread/read",
  "thread/turns/list",
  "thread/items/list",
  "thread/items/read",
  "thread/queue/list",
]);

function readOnly(method: RemoteHumanMethod): boolean {
  return READ_ONLY_METHODS.has(method);
}

interface Pending {
  mutation: boolean;
  result: (value: unknown) => void;
  fail: (error: TransportError) => void;
}

type HandshakeParams = InitializeParams & { token: string };
type ThreadPromptRequest = Extract<
  ServerRequest,
  {
    method:
      | "item/commandExecution/requestApproval"
      | "item/fileChange/requestApproval"
      | "item/tool/requestUserInput"
      | "shellGuard/requestApproval";
  }
>;

function isThreadPromptRequest(request: ServerRequest): request is ThreadPromptRequest {
  return (
    request.method === "item/commandExecution/requestApproval" ||
    request.method === "item/fileChange/requestApproval" ||
    request.method === "item/tool/requestUserInput" ||
    request.method === "shellGuard/requestApproval"
  );
}

// One physical WebSocket connection speaking native mekugi-app-server JSON-RPC
// through mekugi connector (/api/control, subprotocol mekugi.control.v9).
class Channel {
  readonly ready: Promise<InitializeResponse>;
  readonly closedPromise: Promise<void>;
  #resolveClosed!: () => void;
  #initialized = false;
  #socket: WebSocket;
  #closed = false;
  #next = 0;
  #pending = new Map<string, Pending>();
  #pendingPrompts = new Map<RequestId, ThreadPromptRequest>();
  #retiringPromptThreads = new Map<string, Promise<void>>();
  #onNotification?: NotificationListener;
  #onServerRequest?: ServerRequestListener;

  get closed(): boolean {
    return this.#closed;
  }

  constructor(
    endpoint: string,
    token: string,
    onNotification?: NotificationListener,
    onServerRequest?: ServerRequestListener,
  ) {
    this.#onNotification = onNotification;
    this.#onServerRequest = onServerRequest;
    this.closedPromise = new Promise<void>((resolve) => {
      this.#resolveClosed = resolve;
    });
    const url = new URL(endpoint + "/api/control");
    url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
    this.#socket = new WebSocket(url, subprotocol);
    this.#socket.addEventListener("message", (message) => this.#receive(message.data));
    this.ready = new Promise<InitializeResponse>((resolve, reject) => {
      const timer = setTimeout(() => this.close(), 10_000);
      const fail = () => {
        clearTimeout(timer);
        reject(new TransportError(unreachable, "network"));
      };
      this.#socket.addEventListener("close", fail, { once: true });
      this.#socket.addEventListener(
        "open",
        () => {
          if (this.#socket.protocol !== subprotocol) {
            clearTimeout(timer);
            reject(
              new TransportError(
                "This connector uses an incompatible Mekugi protocol.",
                "incompatible",
              ),
            );
            this.close();
            return;
          }
          const params: HandshakeParams = {
            clientInfo: { name: "mekugi-webui", title: "Mekugi WebUI", version: protocolVersion },
            capabilities: {
              experimentalApi: true,
              requiredHumanApproval: true,
              requestAttestation: false,
              optOutNotificationMethods: null,
            },
            token,
          };
          void this.#dispatch<InitializeResponse>("initialize", params)
            .then((result) => {
              if (!result || typeof result.userAgent !== "string")
                throw new TransportError(
                  "This connector uses an incompatible Mekugi protocol.",
                  "incompatible",
                );
              this.#initialized = true;
              const initialized: ClientNotification = { method: "initialized" };
              this.#sendRaw({ jsonrpc: "2.0", ...initialized });
              clearTimeout(timer);
              this.#socket.removeEventListener("close", fail);
              resolve(result);
            })
            .catch((error) => {
              clearTimeout(timer);
              this.#socket.removeEventListener("close", fail);
              reject(
                error instanceof TransportError
                  ? error
                  : new TransportError(unreachable, "network"),
              );
              this.close();
            });
        },
        { once: true },
      );
    });
    this.#socket.addEventListener("error", () => this.close());
    this.#socket.addEventListener("close", () => this.close());
    void this.ready.catch(() => {});
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#socket.close();
    for (const pending of this.#pending.values())
      pending.fail(
        new TransportError(
          pending.mutation ? unknownOutcome : unreachable,
          pending.mutation ? "unknown" : "network",
        ),
      );
    this.#pending.clear();
    this.#pendingPrompts.clear();
    this.#retiringPromptThreads.clear();
    this.#resolveClosed();
  }

  async request<T>(method: RemoteHumanMethod, params?: unknown, signal?: AbortSignal): Promise<T> {
    await waitFor(this.ready, signal);
    if (signal?.aborted) throw canceled();
    return this.#dispatch<T>(method, params, signal);
  }

  respond(id: RequestId, result: unknown): void {
    this.#pendingPrompts.delete(id);
    this.#sendRaw({ jsonrpc: "2.0", id, result });
  }

  respondError(id: RequestId, code: number, message: string): void {
    this.#pendingPrompts.delete(id);
    this.#sendRaw({ jsonrpc: "2.0", id, error: { code, message } });
  }

  replayServerRequests(listener: ServerRequestListener): void {
    for (const request of this.#pendingPrompts.values()) listener(request);
  }

  retireThreadPrompts(threadId: string, done: Promise<void>): void {
    // Teardown can wait on an in-flight resume. Drop that subscription's
    // prompts now and ignore any it still delivers before teardown completes.
    this.#retiringPromptThreads.set(threadId, done);
    this.#forgetThreadPrompts(threadId);
    const finish = () => {
      if (this.#retiringPromptThreads.get(threadId) === done)
        this.#retiringPromptThreads.delete(threadId);
    };
    void done.then(finish, finish);
  }

  #forgetThreadPrompts(threadId: string): void {
    for (const [id, request] of this.#pendingPrompts) {
      if (request.params.threadId === threadId) this.#pendingPrompts.delete(id);
    }
  }

  #sendRaw(payload: unknown): void {
    if (this.#closed || this.#socket.readyState !== WebSocket.OPEN) return;
    try {
      this.#socket.send(JSON.stringify(payload));
    } catch {
      this.close();
    }
  }

  #dispatch<T>(method: RemoteHumanMethod, params?: unknown, signal?: AbortSignal): Promise<T> {
    if (signal?.aborted) return Promise.reject(canceled());
    if (this.#closed || this.#socket.readyState !== WebSocket.OPEN)
      return Promise.reject(new TransportError(unreachable, "network"));
    if (this.#pending.size >= 32 || this.#socket.bufferedAmount > 48 * 1024 * 1024)
      return Promise.reject(
        new TransportError(
          "This connection is busy. Wait for its pending requests.",
          "refused",
          503,
        ),
      );
    const id = String(++this.#next);
    const encoded = JSON.stringify(
      params === undefined
        ? { jsonrpc: "2.0", id, method }
        : { jsonrpc: "2.0", id, method, params },
    );
    const mutation = !readOnly(method);
    return new Promise<T>((resolve, reject) => {
      const finish = () => {
        clearTimeout(timer);
        signal?.removeEventListener("abort", abort);
        this.#pending.delete(id);
      };
      const abort = () => {
        finish();
        reject(mutation ? new TransportError(unknownOutcome, "unknown") : canceled());
      };
      const timer = setTimeout(abort, mutation ? 120_000 : 15_000);
      this.#pending.set(id, {
        mutation,
        result: (value) => {
          finish();
          resolve(value as T);
        },
        fail: (error) => {
          finish();
          reject(error);
        },
      });
      signal?.addEventListener("abort", abort, { once: true });
      try {
        this.#socket.send(encoded);
      } catch {
        this.close();
      }
    });
  }

  #receive(data: unknown): void {
    if (this.#closed) return;
    let message: WireMessage;
    try {
      if (typeof data !== "string" || data.length > 64 * 1024 * 1024)
        throw new Error("Invalid frame");
      message = JSON.parse(data) as WireMessage;
      if (!message || typeof message !== "object" || Array.isArray(message))
        throw new Error("Invalid RPC");
      // mekugi-app-server omits "jsonrpc" while connector refusals include "jsonrpc": "2.0".
      if (message.jsonrpc !== undefined && message.jsonrpc !== "2.0")
        throw new Error("Invalid RPC version");
      const hasId = typeof message.id === "string" || typeof message.id === "number";
      const hasMethod = typeof message.method === "string";
      if (!this.#initialized && (!hasId || String(message.id) !== "1"))
        throw new Error("Message before initialization");

      // Server -> Client request (has both id and method)
      if (hasId && hasMethod) {
        const request = {
          id: message.id!,
          method: message.method!,
          params: message.params,
        } as ServerRequest;
        // Native resume may resend a request that this channel already delivered.
        if (isThreadPromptRequest(request)) {
          if (
            this.#retiringPromptThreads.has(request.params.threadId) ||
            this.#pendingPrompts.has(request.id)
          )
            return;
          this.#pendingPrompts.set(request.id, request);
        }
        this.#onServerRequest?.(request);
        return;
      }

      // Response to a Client -> Server request (has id, no method)
      if (hasId) {
        const pending = this.#pending.get(String(message.id));
        if (!pending) return;
        if (message.error) pending.fail(remoteError(message.error, String(message.id) === "1"));
        else if ("result" in message) pending.result(message.result);
        else throw new Error("Missing result");
        return;
      }

      // Server -> Client notification (has method, no id)
      if (hasMethod) {
        const notification = {
          method: message.method!,
          params: message.params,
        } as ServerNotification;
        if (notification.method === "serverRequest/resolved") {
          const { requestId, threadId } = notification.params;
          if (this.#pendingPrompts.get(requestId)?.params.threadId === threadId)
            this.#pendingPrompts.delete(requestId);
        } else if (
          notification.method === "thread/closed" ||
          notification.method === "thread/archived" ||
          notification.method === "thread/deleted"
        ) {
          this.#forgetThreadPrompts(notification.params.threadId);
        }
        this.#onNotification?.(notification);
        return;
      }

      throw new Error("Unexpected RPC message");
    } catch {
      this.close();
    }
  }
}

function waitFor<T>(promise: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return promise;
  if (signal.aborted) return Promise.reject(canceled());
  return new Promise((resolve, reject) => {
    const abort = () => reject(canceled());
    signal.addEventListener("abort", abort, { once: true });
    promise.then(resolve, reject).finally(() => signal.removeEventListener("abort", abort));
  });
}

function attachmentBearing(method: RemoteHumanMethod, params: unknown): boolean {
  if (method !== "turn/start" && method !== "turn/steer") return false;
  const input = (params as TurnStartParams | TurnSteerParams | undefined)?.input;
  return !!input?.some((item) => item.type === "image");
}

export class Connector {
  readonly endpoint: string;
  #token: string;
  #lifetime = new AbortController();
  #disposed = false;
  #shared?: Channel;
  #bulk?: Channel;
  #info?: InitializeResponse;
  #notifications = new Set<NotificationListener>();
  #serverRequests = new Set<ServerRequestListener>();

  get info(): InitializeResponse | undefined {
    return this.#info;
  }

  constructor(endpoint: string, token: string) {
    this.endpoint = endpointURL(endpoint);
    this.#token = this.#credential(token);
  }

  #credential(token: string): string {
    const value = token.trim();
    if (!value || /\s/.test(value)) throw new Error("Enter this machine's bearer token.");
    return value;
  }

  authorize(token: string): void {
    const value = this.#credential(token);
    if (this.#disposed) throw new Error("This machine connection was removed.");
    const previous = this.#lifetime;
    this.#token = value;
    this.#lifetime = new AbortController();
    previous.abort();
    this.#shared?.close();
    this.#shared = undefined;
    this.#bulk?.close();
    this.#bulk = undefined;
  }

  close(): void {
    this.#disposed = true;
    this.#token = "";
    this.#lifetime.abort();
    this.#shared?.close();
    this.#bulk?.close();
    this.#notifications.clear();
    this.#serverRequests.clear();
  }

  onNotification(listener: NotificationListener): () => void {
    this.#notifications.add(listener);
    return () => this.#notifications.delete(listener);
  }

  onServerRequest(listener: ServerRequestListener): () => void {
    this.#serverRequests.add(listener);
    this.#shared?.replayServerRequests(listener);
    return () => this.#serverRequests.delete(listener);
  }

  retireThreadPrompts(threadId: string, done: Promise<void>): void {
    if (this.#shared && !this.#shared.closed) this.#shared.retireThreadPrompts(threadId, done);
  }

  respond(id: RequestId, result: unknown): void {
    this.#shared?.respond(id, result);
  }

  respondError(id: RequestId, code: number, message: string): void {
    this.#shared?.respondError(id, code, message);
  }

  #channel(): Channel {
    if (!this.#shared || this.#shared.closed) {
      this.#shared = new Channel(
        this.endpoint,
        this.#token,
        (notification) => {
          for (const listener of this.#notifications) listener(notification);
        },
        (request) => {
          for (const listener of this.#serverRequests) listener(request);
        },
      );
    }
    return this.#shared;
  }

  connect(state: StateListener, onOpen?: OpenListener): () => void {
    const controller = new AbortController();
    const run = async () => {
      let attempt = 0;
      while (!controller.signal.aborted && !this.#disposed) {
        const lifetime = this.#lifetime.signal;
        const signal = AbortSignal.any([controller.signal, lifetime]);
        state("connecting");
        try {
          const channel = this.#channel();
          const info = await waitFor(channel.ready, signal);
          if (signal.aborted) continue;
          this.#info = info;
          attempt = 0;
          state("open");
          onOpen?.(info);
          await waitFor(channel.closedPromise, signal);
          if (controller.signal.aborted || this.#disposed) return;
          if (lifetime !== this.#lifetime.signal) continue;
          state("lost", new TransportError(unreachable, "network"));
          await pause(backoff(attempt++), signal);
        } catch (error) {
          if (controller.signal.aborted || this.#disposed) return;
          if (lifetime !== this.#lifetime.signal) continue;
          const failure =
            error instanceof TransportError ? error : new TransportError(unreachable, "network");
          const blocked = failure.kind === "required" || failure.kind === "incompatible";
          state(blocked ? (failure.kind as "required" | "incompatible") : "lost", failure);
          await pause(blocked ? undefined : backoff(attempt++), signal);
        }
      }
    };
    void run();
    return () => controller.abort();
  }

  async request<T>(
    method: RemoteHumanMethod,
    params?: unknown,
    options: RequestOptions = {},
  ): Promise<T> {
    if (this.#disposed || options.signal?.aborted) throw canceled();
    const signal = AbortSignal.any([
      this.#lifetime.signal,
      ...(options.signal ? [options.signal] : []),
    ]);
    if (!attachmentBearing(method, params))
      return this.#channel().request<T>(method, params, signal);
    if (!this.#bulk || this.#bulk.closed) this.#bulk = new Channel(this.endpoint, this.#token);
    return this.#bulk.request<T>(method, params, signal);
  }

  async resource<T>(
    kind: ResourceKind,
    params: ResourceRequest,
    options: RequestOptions = {},
  ): Promise<T> {
    if (this.#disposed || options.signal?.aborted) throw canceled();
    const signal = AbortSignal.any([
      this.#lifetime.signal,
      ...(options.signal ? [options.signal] : []),
    ]);
    try {
      const response = await fetch(`${this.endpoint}/api/resources/${kind}`, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${this.#token}`,
          "Content-Type": "application/json",
          "Mekugi-Protocol": protocolVersion,
        },
        body: JSON.stringify(params),
        credentials: "omit",
        redirect: "error",
        cache: "no-store",
        signal,
      });
      if (!response.ok) throw await resourceError(response);
      const json =
        kind === "content" ||
        kind === "file" ||
        (kind === "attachment-image" && params.body && "probe" in params.body && params.body.probe);
      return (await (json ? response.json() : response.blob())) as T;
    } catch (error) {
      if (signal.aborted) throw canceled();
      if (error instanceof TransportError) throw error;
      throw new TransportError(unreachable, "network");
    }
  }
}

async function resourceError(response: Response): Promise<TransportError> {
  if (response.headers.get("Content-Type")?.includes("application/json"))
    return remoteError((await response.json()) as RPCErrorPayload);
  if (response.status === 401)
    return new TransportError("This machine requires a valid bearer token.", "required", 401);
  if (response.status === 409)
    return new TransportError(
      "This connector uses an incompatible Mekugi protocol.",
      "incompatible",
      409,
    );
  return new TransportError(
    "The resource could not be read from this machine.",
    "refused",
    response.status,
  );
}

function pause(milliseconds: number | undefined, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const finish = () => {
      clearTimeout(timer);
      signal.removeEventListener("abort", finish);
      resolve();
    };
    if (signal.aborted) return finish();
    signal.addEventListener("abort", finish, { once: true });
    if (milliseconds !== undefined) timer = setTimeout(finish, milliseconds);
  });
}

export interface Selection {
  machine?: string;
  session?: string;
  agent?: string;
}

export function selectionHash(selection: Selection): string {
  const params = new URLSearchParams();
  if (selection.machine) params.set("machine", selection.machine);
  if (selection.machine && selection.session) params.set("thread", selection.session);
  if (selection.machine && selection.session && selection.agent && selection.agent !== "/root")
    params.set("agent", selection.agent);
  return params.size ? `#${params}` : "";
}

export function parseSelection(hash: string): Selection {
  const params = new URLSearchParams(hash.replace(/^#/, ""));
  const machine = params.get("machine") || undefined;
  const session = machine ? params.get("thread") || undefined : undefined;
  const agent = session ? params.get("agent") || undefined : undefined;
  return { machine, session, agent: agent === "/root" ? undefined : agent };
}
