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
  ThreadListParams,
  ThreadListResponse,
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
type ThreadListing = Listed & { id: string };
type ListingChange = ThreadListing | Partial<Pick<Listed, "title" | "status" | "waiting">> | null;
const storagePrefix = "mekugi.machine.v1:";

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

function listedActivity(status: ThreadStatus): Pick<Listed, "status" | "waiting"> {
  const waiting =
    status.type === "active" &&
    (status.activeFlags.includes("waitingOnApproval") ||
      status.activeFlags.includes("waitingOnUserInput"))
      ? 1
      : undefined;
  return { status: threadStatusToStatus(status), waiting };
}

export function threadToListed(thread: Thread): ThreadListing {
  return {
    id: thread.id,
    thread: thread.id,
    parentThread: thread.parentThreadId ?? undefined,
    title: thread.name || thread.preview || undefined,
    workspace: thread.cwd || undefined,
    started: thread.createdAt ? thread.createdAt * 1000 : undefined,
    ...listedActivity(thread.status),
  };
}

function standaloneThread(thread: Thread): boolean {
  return !["mekugi_btw", "mekugi_side", "mekugi_side_question"].includes(thread.threadSource ?? "");
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
  sessions = $state.raw<ThreadListing[] | undefined>();
  connection = $state<Connection>("connecting");
  error = $state<string | undefined>();
  info = $state.raw<InitializeResponse | undefined>();
  models = $state.raw<Model[]>([]);
  #connector: Connector;
  #token: string;
  #stop?: () => void;
  #stopNotifications?: () => void;
  #threadRefresh?: { controller: AbortController; changes: Map<string, ListingChange> };
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

  retireThreadPrompts(threadId: string, done: Promise<void>): void {
    this.#connector.retireThreadPrompts(threadId, done);
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
    this.#threadRefresh?.controller.abort();
    const controller = new AbortController();
    const changes = new Map<string, ListingChange>();
    this.#threadRefresh = { controller, changes };
    try {
      const all = new Map<string, ThreadListing>();
      const params: ThreadListParams = {
        limit: 100,
        archived: false,
        useStateDbOnly: true,
        modelProviders: [],
        sourceKinds: [
          "cli",
          "vscode",
          "exec",
          "appServer",
          "subAgent",
          "subAgentReview",
          "subAgentCompact",
          "subAgentThreadSpawn",
          "subAgentOther",
          "unknown",
        ],
      };
      let cursor: string | undefined;
      do {
        const response = await this.request<ThreadListResponse>(
          "thread/list",
          { ...params, ...(cursor ? { cursor } : {}) } satisfies ThreadListParams,
          { signal: controller.signal },
        );
        if (controller.signal.aborted) return;
        cursor = response.nextCursor ?? undefined;
        const previousSize = all.size;
        for (const thread of response.data ?? []) {
          if (standaloneThread(thread) && !all.has(thread.id)) {
            all.set(thread.id, threadToListed(thread));
          }
        }
        if (cursor && all.size === previousSize) continue;

        // Keep live starts not yet in the snapshot and retain the old tail until the final page.
        const next: ThreadListing[] = [];
        const stale: ThreadListing[] = [];
        for (const listed of this.sessions ?? []) {
          if (all.has(listed.id)) continue;
          const change = changes.get(listed.id);
          if (change && "id" in change) next.push(listed);
          else if (cursor) stale.push(listed);
        }
        for (const [id, listed] of all) {
          const change = changes.get(id);
          if (change !== null) next.push(change ? { ...listed, ...change } : listed);
        }
        if (!cursor || next.length || stale.length) {
          this.sessions = stale.length ? next.concat(stale) : next;
        }
      } while (cursor);
    } catch {
      // Transient refresh failures are surfaced through the connection state.
    } finally {
      if (this.#threadRefresh?.controller === controller) this.#threadRefresh = undefined;
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
    this.#threadRefresh?.controller.abort();
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

  #changeListing(threadId: string, change: ListingChange): void {
    const changes = this.#threadRefresh?.changes;
    if (changes) {
      const previous = changes.get(threadId);
      if (!change || "id" in change) changes.set(threadId, change);
      else if (previous !== null) changes.set(threadId, { ...previous, ...change });
    }

    const current = this.sessions ?? [];
    const index = current.findIndex((s) => s.id === threadId);
    if (index < 0) {
      if (change && "id" in change) this.sessions = [change, ...current];
      return;
    }
    const next = current.slice();
    if (!change) next.splice(index, 1);
    else next[index] = "id" in change ? change : { ...next[index], ...change };
    this.sessions = next;
  }

  #handleNotification(notification: ServerNotification): void {
    switch (notification.method) {
      case "thread/started": {
        if (!standaloneThread(notification.params.thread)) break;
        const listed = threadToListed(notification.params.thread);
        this.#changeListing(listed.id, listed);
        break;
      }
      case "thread/status/changed": {
        const { threadId, status } = notification.params;
        this.#changeListing(threadId, listedActivity(status));
        break;
      }
      case "thread/name/updated": {
        const { threadId, threadName } = notification.params;
        if (threadName) this.#changeListing(threadId, { title: threadName });
        break;
      }
      case "thread/archived":
      case "thread/deleted": {
        this.#changeListing(notification.params.threadId, null);
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
    this.#threadRefresh?.controller.abort();
    this.#token = token.trim();
    this.connection = "connecting";
    this.error = undefined;
  }

  close(): void {
    this.#threadRefresh?.controller.abort();
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
      return {
        options: {
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
  ): Promise<{ response: ThreadStartResponse } | { error: string }> {
    try {
      const config = start.effort ? { model_reasoning_effort: start.effort } : null;
      const params: ThreadStartParams = {
        cwd: start.workspace || null,
        model: start.model || null,
        ...(config ? { config } : {}),
      };
      const response = await machine.request<ThreadStartResponse>("thread/start", params, {
        signal,
      });
      return { response };
    } catch (error) {
      return { error: errorText(error, true) };
    }
  }
}

export const hub = new Hub();
