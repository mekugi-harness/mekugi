import type {
  InitializeResponse,
  Listed,
  Model,
  ModelGroup,
  ModelListResponse,
  RemoteHumanMethod,
  RequestId,
  ResourceRequest,
  ServerNotification,
  ServerRequest,
  Start,
  StartModel,
  StartOptions,
  Status,
  Thread,
  ThreadArchiveParams,
  ThreadListResponse,
  ThreadResumeParams,
  ThreadResumeResponse,
  ThreadStartParams,
  ThreadStartResponse,
  ThreadStatus,
} from "./protocol";
import {
  Connector,
  errorText,
  newIdentity,
  type Connection,
  type RequestOptions,
  type ResourceKind,
} from "./transport";
import { select, view } from "./view.svelte";

type SavedMachine = { id: string; label: string; endpoint: string; token: string };
const storagePrefix = "mekugi.machine.v1:";
function normalizeEndpoint(raw: string): string | undefined {
  try {
    const u = new URL(raw.trim());
    if (u.protocol !== "http:" && u.protocol !== "https:") return undefined;
    return `${u.origin}${u.pathname.replace(/\/+$/, "")}`;
  } catch {
    return undefined;
  }
}

export function threadStatusToStatus(status: ThreadStatus): Status {
  switch (status.type) {
    case "active": {
      const waitingApproval = status.activeFlags.includes("waitingOnApproval");
      const waitingInput = status.activeFlags.includes("waitingOnUserInput");
      return {
        running: true,
        text: waitingApproval
          ? "Waiting for approval"
          : waitingInput
            ? "Waiting for input"
            : "Working",
      };
    }
    case "systemError":
      return { running: false, alert: true, text: "System error" };
    case "idle":
      return { running: false, text: "Idle" };
    case "notLoaded":
      return { running: false };
  }
}

export function threadToListed(thread: Thread): Listed {
  const status = threadStatusToStatus(thread.status);
  const waiting =
    thread.status.type === "active" &&
    (thread.status.activeFlags.includes("waitingOnApproval") ||
      thread.status.activeFlags.includes("waitingOnUserInput"))
      ? 1
      : 0;
  return {
    id: thread.id,
    thread: thread.id,
    parentThread: thread.parentThreadId ?? undefined,
    title: thread.name || thread.preview || undefined,
    workspace: thread.cwd || undefined,
    started: thread.createdAt ? thread.createdAt * 1000 : undefined,
    status,
    waiting: waiting || undefined,
  };
}

function modelsToGroups(models: Model[]): { groups: ModelGroup[]; defaultModel: string } {
  const visible = models.filter((m) => !m.hidden);
  const list = visible.length ? visible : models;
  const startModels: StartModel[] = list.map((m) => ({
    id: m.id || m.model,
    label: m.displayName || m.model || m.id,
    efforts: m.supportedReasoningEfforts?.map((e) => e.reasoningEffort) ?? [],
    defaultEffort: m.defaultReasoningEffort,
    serviceTiers: m.serviceTiers?.map((t) => t.id) ?? [],
    defaultServiceTier: m.defaultServiceTier ?? undefined,
  }));
  const defaultEntry = list.find((m) => m.isDefault) ?? list[0];
  const defaultModel = defaultEntry ? defaultEntry.id || defaultEntry.model : "";
  return {
    groups: startModels.length ? [{ label: "Models", models: startModels }] : [],
    defaultModel,
  };
}

export class Machine {
  readonly id: string;
  readonly label: string;
  readonly endpoint: string;
  sessions = $state.raw<Listed[] | undefined>();
  connection = $state<Connection>("connecting");
  error = $state<string | undefined>();
  latencyMs = $state<number | undefined>();
  info = $state.raw<InitializeResponse | undefined>();
  models = $state.raw<Model[]>([]);
  #connector: Connector;
  #token: string;
  #stop?: () => void;
  #stopNotifications?: () => void;
  #openListeners = new Set<(info: InitializeResponse) => void>();

  constructor(label: string, endpoint: string, token: string, id = newIdentity()) {
    this.id = id;
    this.label = label.trim();
    if (!this.label) throw new Error("Enter a machine label.");
    this.#connector = new Connector(endpoint, token);
    this.endpoint = this.#connector.endpoint;
    this.#token = token.trim();
  }

  saved(): SavedMachine {
    return { id: this.id, label: this.label, endpoint: this.endpoint, token: this.#token };
  }

  request<T>(method: RemoteHumanMethod, params?: unknown, options?: RequestOptions): Promise<T> {
    return this.#connector.request<T>(method, params, options);
  }

  resource<T>(kind: ResourceKind, params: ResourceRequest, options?: RequestOptions): Promise<T> {
    return this.#connector.resource<T>(kind, params, options);
  }

  onNotification(listener: (notification: ServerNotification) => void): () => void {
    return this.#connector.onNotification(listener);
  }

  onServerRequest(listener: (request: ServerRequest) => void): () => void {
    return this.#connector.onServerRequest(listener);
  }

  onOpen(listener: (info: InitializeResponse) => void): () => void {
    this.#openListeners.add(listener);
    return () => this.#openListeners.delete(listener);
  }

  respond(id: RequestId, result: unknown): void {
    this.#connector.respond(id, result);
  }

  respondError(id: RequestId, code: number, message: string): void {
    this.#connector.respondError(id, code, message);
  }

  async refreshThreads(): Promise<void> {
    try {
      const all: Listed[] = [];
      let cursor: string | undefined;
      for (let page = 0; page < 10; page++) {
        const pageStart = page === 0 ? performance.now() : 0;
        const response = await this.request<ThreadListResponse>("thread/list", {
          limit: 100,
          archived: false,
          ...(cursor ? { cursor } : {}),
        });
        if (page === 0) {
          this.latencyMs = Math.max(1, Math.round(performance.now() - pageStart));
        }
        for (const thread of response.data ?? []) all.push(threadToListed(thread));
        const next = response.nextCursor ?? undefined;
        if (!next || next === cursor) break;
        cursor = next;
      }
      this.sessions = all;
    } catch {
      // Transient refresh failures are surfaced through the connection state.
    }
  }

  async refreshModels(): Promise<void> {
    try {
      const response = await this.request<ModelListResponse>("model/list", {
        includeHidden: false,
      });
      this.models = response.data ?? [];
    } catch {
      // Model catalog refresh is optional.
    }
  }

  start(): void {
    this.#stop?.();
    this.#stopNotifications?.();
    this.#stopNotifications = this.onNotification((notification) => {
      this.#handleNotification(notification);
    });
    this.#stop = this.#connector.connect(
      (connection, error) => {
        if (connection === "connecting" && this.connection === "lost") return;
        this.connection = connection;
        this.error = error?.message;
      },
      (info) => {
        this.info = info;
        void this.refreshThreads();
        for (const listener of this.#openListeners) listener(info);
      },
    );
  }

  #handleNotification(notification: ServerNotification): void {
    switch (notification.method) {
      case "thread/started": {
        const listed = threadToListed(notification.params.thread);
        const current = this.sessions ?? [];
        const index = current.findIndex((s) => s.id === listed.id);
        this.sessions =
          index >= 0 ? current.map((s, i) => (i === index ? listed : s)) : [listed, ...current];
        break;
      }
      case "thread/status/changed": {
        const { threadId, status } = notification.params;
        if (!this.sessions) return;
        const mapped = threadStatusToStatus(status);
        const waiting =
          status.type === "active" &&
          (status.activeFlags.includes("waitingOnApproval") ||
            status.activeFlags.includes("waitingOnUserInput"))
            ? 1
            : undefined;
        this.sessions = this.sessions.map((s) =>
          s.id === threadId ? { ...s, status: mapped, waiting } : s,
        );
        break;
      }
      case "thread/name/updated": {
        const { threadId, threadName } = notification.params;
        if (!this.sessions) return;
        this.sessions = this.sessions.map((s) =>
          s.id === threadId ? { ...s, title: threadName || s.title } : s,
        );
        break;
      }
      case "thread/archived":
      case "thread/deleted": {
        const { threadId } = notification.params;
        if (!this.sessions) return;
        this.sessions = this.sessions.filter((s) => s.id !== threadId);
        break;
      }
      case "thread/unarchived": {
        void this.refreshThreads();
        break;
      }
      default:
        break;
    }
  }

  // What the machine's connection means to someone choosing where to work.
  get state(): { text: string; tone: "open" | "warn" | "alert" | "" } {
    switch (this.connection) {
      case "open":
        return { text: "Connected", tone: "open" };
      case "lost":
        return { text: "Unreachable, retrying", tone: "warn" };
      case "required":
        return { text: "Token rejected", tone: "alert" };
      case "incompatible":
        return { text: "Incompatible version", tone: "alert" };
      default:
        return { text: "Connecting…", tone: "" };
    }
  }

  authorize(token: string): void {
    this.#connector.authorize(token);
    this.#token = token.trim();
    this.connection = "connecting";
    this.error = undefined;
  }

  close(): void {
    this.#stop?.();
    this.#stopNotifications?.();
    this.#openListeners.clear();
    this.#connector.close();
    this.#token = "";
  }
}

class Hub {
  machines = $state.raw<Machine[]>([]);
  storageError = $state<string>();
  #restoreError?: string;
  #failedWrites = new Set<string>();

  constructor() {
    try {
      for (let index = 0; index < localStorage.length; index++) {
        const key = localStorage.key(index);
        if (!key?.startsWith(storagePrefix)) continue;
        try {
          const saved = localStorage.getItem(key);
          if (saved === null) continue;
          const record = JSON.parse(saved);
          if (
            !record ||
            typeof record.id !== "string" ||
            !record.id.trim() ||
            key !== storagePrefix + record.id ||
            typeof record.label !== "string" ||
            typeof record.endpoint !== "string" ||
            typeof record.token !== "string"
          )
            throw new Error("Invalid saved machine");
          this.machines.push(new Machine(record.label, record.endpoint, record.token, record.id));
        } catch {
          this.storageError =
            "Some saved machines could not be restored. Add them again under Machines.";
        }
      }
    } catch {
      this.storageError =
        "Saved machines could not be read. Check this site's browser storage settings.";
    }
    this.#restoreError = this.storageError;
    for (const machine of this.machines) machine.start();
  }

  #save(id: string, machine?: Machine): void {
    try {
      if (machine) localStorage.setItem(storagePrefix + id, JSON.stringify(machine.saved()));
      else localStorage.removeItem(storagePrefix + id);
      this.#failedWrites.delete(id);
    } catch {
      this.#failedWrites.add(id);
    }
    this.storageError = this.#failedWrites.size
      ? "Machine changes could not be saved. Reloading may restore older machines or tokens. Check this site's browser storage settings."
      : this.#restoreError;
  }

  get selected(): Machine | undefined {
    return this.machines.find((machine) => machine.id === view.machine);
  }

  async add(label: string, endpoint: string, token: string): Promise<string | undefined> {
    try {
      const machine = new Machine(label, endpoint, token);
      this.machines = [...this.machines, machine];
      this.#save(machine.id, machine);
      machine.start();
      if (!view.session) select(machine.id);
      return undefined;
    } catch (error) {
      return error instanceof Error ? error.message : "Could not add this machine.";
    }
  }

  async importFleetJson(raw: string): Promise<{ added: number; skipped: number; error?: string }> {
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      return { added: 0, skipped: 0, error: "Invalid DEVBOXES_JSON syntax." };
    }
    const entries: unknown[] = Array.isArray(parsed)
      ? parsed
      : parsed &&
          typeof parsed === "object" &&
          "boxes" in parsed &&
          Array.isArray((parsed as { boxes: unknown[] }).boxes)
        ? (parsed as { boxes: unknown[] }).boxes
        : parsed &&
            typeof parsed === "object" &&
            "nodes" in parsed &&
            Array.isArray((parsed as { nodes: unknown[] }).nodes)
          ? (parsed as { nodes: unknown[] }).nodes
          : [];
    if (!entries.length) {
      return { added: 0, skipped: 0, error: "No devbox entries found in DEVBOXES_JSON." };
    }
    let added = 0;
    let skipped = 0;
    const existing = new Set(
      this.machines
        .map((m) => normalizeEndpoint(m.endpoint))
        .filter((u): u is string => u !== undefined),
    );
    for (const item of entries) {
      if (!item || typeof item !== "object") {
        skipped++;
        continue;
      }
      const rec = item as Record<string, unknown>;
      const label =
        typeof rec.name === "string" ? rec.name : typeof rec.label === "string" ? rec.label : "";
      const endpoint =
        typeof rec.endpoint === "string"
          ? rec.endpoint
          : typeof rec.url === "string"
            ? rec.url
            : typeof rec.target === "string"
              ? rec.target
              : "";
      const token = typeof rec.token === "string" ? rec.token : "";
      const key = normalizeEndpoint(endpoint);
      if (!label.trim() || !key || !token.trim() || existing.has(key)) {
        skipped++;
        continue;
      }
      const err = await this.add(label, endpoint, token);
      if (err) {
        skipped++;
      } else {
        existing.add(key);
        added++;
      }
    }
    return { added, skipped };
  }

  remove(id: string): void {
    this.machines.find((machine) => machine.id === id)?.close();
    this.machines = this.machines.filter((machine) => machine.id !== id);
    this.#save(id);
    if (view.machine === id) select(undefined);
  }

  authorize(id: string, token: string): string | undefined {
    const machine = this.machines.find((candidate) => candidate.id === id);
    if (!machine) return "This machine connection was removed.";
    try {
      machine.authorize(token);
      this.#save(id, machine);
      return undefined;
    } catch (error) {
      return error instanceof Error
        ? error.message
        : "Could not replace this machine's credential.";
    }
  }

  async startOptions(machine: Machine): Promise<{ options: StartOptions } | { error: string }> {
    try {
      const response = await machine.request<ModelListResponse>("model/list", {
        includeHidden: false,
      });
      const models = response.data ?? [];
      machine.models = models;
      const { groups, defaultModel } = modelsToGroups(models);
      const workspaces = [
        ...new Set(
          (machine.sessions ?? []).map((s) => s.workspace).filter((w): w is string => !!w),
        ),
      ];
      return {
        options: {
          workspaces,
          models: groups,
          defaultModel,
        },
      };
    } catch (error) {
      return { error: errorText(error) };
    }
  }

  async createSession(
    start: Start,
    machine: Machine,
    signal?: AbortSignal,
  ): Promise<{ id: string } | { error: string }> {
    try {
      const config = start.effort ? { model_reasoning_effort: start.effort } : null;
      if (start.resume) {
        const params: ThreadResumeParams = {
          threadId: start.resume,
          cwd: start.workspace || null,
          model: start.model || null,
          ...(config ? { config } : {}),
        };
        const response = await machine.request<ThreadResumeResponse>("thread/resume", params, {
          signal,
        });
        return { id: response.thread.id };
      }
      const params: ThreadStartParams = {
        cwd: start.workspace || null,
        model: start.model || null,
        ...(config ? { config } : {}),
      };
      const response = await machine.request<ThreadStartResponse>("thread/start", params, {
        signal,
      });
      return { id: response.thread.id };
    } catch (error) {
      return { error: errorText(error, true) };
    }
  }

  async endSession(id: string, machine: Machine): Promise<string | undefined> {
    try {
      const params: ThreadArchiveParams = { threadId: id };
      await machine.request("thread/archive", params);
      if (machine.sessions) {
        machine.sessions = machine.sessions.filter((s) => s.id !== id);
      }
      return undefined;
    } catch (error) {
      return errorText(error, true);
    }
  }
}

export const hub = new Hub();
