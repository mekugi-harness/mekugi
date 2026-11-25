import { roleColor } from "./agents";
import { threadStatusToStatus, type Machine } from "./hub.svelte";
import { errorText, TransportError, type Connection } from "./transport";
import type {
  Agent,
  AgentIdentity,
  AgentState,
  Change,
  ContentReference,
  ContentResourceResponse,
  Entry,
  Input,
  Preview,
  ReviewedFile,
  Roster,
  SavedReview,
  ServerNotification,
  Session,
  Status,
  Thread,
  ThreadChangesReadResponse,
  ThreadItem,
  ThreadListResponse,
  ThreadReadResponse,
  ThreadResumeResponse,
  ThreadStatus,
  Turn,
  Value,
} from "./protocol";

export type { Connection } from "./transport";

export interface Feed {
  attachment?: string;
  order: number[];
  entries: Record<number, Entry>;
  revision?: number;
}

export interface Gone {
  status: number;
  text: string;
}

const empty: Feed = { order: [], entries: {} };

function threadStatusToAgentState(status: ThreadStatus): AgentState {
  switch (status.type) {
    case "active":
      return status.activeFlags.includes("waitingOnApproval") ||
        status.activeFlags.includes("waitingOnUserInput")
        ? "waiting"
        : "working";
    case "systemError":
      return "error";
    case "idle":
      return "idle";
    case "notLoaded":
      return "done";
  }
}

function itemToEntry(
  item: ThreadItem,
  seq: number,
  turnIndex: number,
  turnId: string,
  agent = "/root",
): Entry {
  const base: Entry = {
    seq,
    position: seq,
    itemId: item.id,
    turnId,
    agent,
    observed: item.startedAtMs ?? undefined,
    content: { turnId, itemId: item.id },
  };
  switch (item.type) {
    case "userMessage": {
      const parts: string[] = [];
      const images: string[] = [];
      for (const input of item.content) {
        if (input.type === "text") parts.push(input.text);
        else if (input.type === "localImage") images.push(input.path);
        else if (input.type === "image" && "url" in input) images.push(input.url);
        else if (input.type === "skill") parts.push(`$${input.name}`);
        else if (input.type === "mention") parts.push(`@${input.name}`);
      }
      return {
        ...base,
        kind: "user",
        turn: turnIndex,
        text: parts.join("\n"),
        images: images.length ? images : undefined,
      };
    }
    case "agentMessage":
      return {
        ...base,
        kind: "assistant",
        text: item.text,
      };
    case "plan":
      return {
        ...base,
        kind: "plan",
        text: item.text,
      };
    case "reasoning": {
      const text = [...(item.summary ?? []), ...(item.content ?? [])].filter(Boolean).join("\n\n");
      return {
        ...base,
        kind: "thinking",
        text,
      };
    }
    case "commandExecution": {
      const running = item.status === "inProgress";
      const failed = item.status === "failed" || item.status === "declined";
      const tail = item.aggregatedOutput ? item.aggregatedOutput.split("\n") : undefined;
      const value: Value = {
        kind: "shell",
        verb: "run",
        code: item.command,
        workdir: item.cwd || undefined,
        exitCode: item.exitCode,
        duration: item.durationMs ?? undefined,
        running,
        failed,
        tail,
      };
      return {
        ...base,
        kind: "tool",
        running,
        outcome: failed ? "error" : undefined,
        values: [value],
      };
    }
    case "fileChange": {
      const running = item.status === "inProgress";
      const failed = item.status === "failed" || item.status === "declined";
      const changes: Change[] = item.changes.map((c) => ({
        kind: c.kind.type,
        path: c.path,
        note: c.diff ? undefined : c.kind.type,
        new: c.diff ? c.diff.split("\n") : undefined,
      }));
      const value: Value = {
        kind: "edit",
        verb: "edit",
        running,
        failed,
        changes,
      };
      return {
        ...base,
        kind: "tool",
        running,
        outcome: failed ? "error" : undefined,
        values: [value],
      };
    }
    case "mcpToolCall": {
      const running = item.status === "inProgress";
      const failed = item.status === "failed";
      const value: Value = {
        kind: "tool",
        verb: `${item.server}:${item.tool}`,
        running,
        failed,
        duration: item.durationMs ?? undefined,
        details: item.error?.message,
      };
      return {
        ...base,
        kind: "tool",
        running,
        outcome: failed ? "error" : undefined,
        values: [value],
      };
    }
    case "dynamicToolCall": {
      const running = item.status === "inProgress";
      const failed = item.status === "failed" || item.success === false;
      const value: Value = {
        kind: "tool",
        verb: item.tool,
        running,
        failed,
        duration: item.durationMs ?? undefined,
      };
      return {
        ...base,
        kind: "tool",
        running,
        outcome: failed ? "error" : undefined,
        values: [value],
      };
    }
    case "webSearch":
      return {
        ...base,
        kind: "tool",
        values: [{ kind: "search", verb: "search", label: item.query }],
      };
    case "imageView":
      return {
        ...base,
        kind: "tool",
        images: [item.path],
        values: [{ kind: "read", verb: "view", path: item.path }],
      };
    case "subAgentActivity":
      return {
        ...base,
        kind: "agent",
        assignment: !!item.assignment,
        assignmentTo: item.agentPath,
        assignmentText: item.assignment ?? undefined,
        text: item.assignment ?? item.agentPath,
      };
    case "collabAgentToolCall":
      return {
        ...base,
        kind: "agent",
        text: item.prompt ?? item.tool,
      };
    case "contextCompaction":
      return {
        ...base,
        kind: "system",
        text: "Context compacted",
      };
    case "enteredReviewMode":
    case "exitedReviewMode":
      return {
        ...base,
        kind: "system",
        text:
          item.review ||
          (item.type === "enteredReviewMode" ? "Entered review mode" : "Exited review mode"),
      };
    case "hookPrompt":
      return {
        ...base,
        kind: "system",
        text: item.fragments.map((f) => f.text).join("\n"),
      };
    default:
      return {
        ...base,
        kind: "system",
        text: item.type,
      };
  }
}

function buildFeed(turns: Turn[], attachment?: string): Feed {
  const entries: Record<number, Entry> = {};
  const order: number[] = [];
  let seq = 0;
  for (let t = 0; t < turns.length; t++) {
    const turn = turns[t];
    for (const item of turn.items) {
      seq++;
      entries[seq] = itemToEntry(item, seq, t + 1, turn.id);
      order.push(seq);
    }
  }
  return { attachment, order, entries, revision: seq };
}

function changesToReview(response: ThreadChangesReadResponse, revision: number): SavedReview {
  const files: ReviewedFile[] = (response.files ?? []).map((file, index) => ({
    key: `${index}:${file.path}`,
    path: file.path,
    status: file.status,
    chunks: [
      {
        Key: `${index}:${file.path}`,
        Status: file.status,
        Stream: "saved",
        Workspace: response.workspace ?? "",
        CaptureOrder: index,
        Change: response.changes[0]?.changeId ?? "saved",
        Caller: "/root",
        Source: response.changes[0]?.sourceKind ?? "turn",
        Highlighted: false,
        Review: {
          Path: file.path,
          Action: file.status,
          Diff: file.unifiedDiff,
          Binary: file.isBinary,
          Added: file.additions,
          Removed: file.deletions,
        },
      },
    ],
    origins: response.changes
      .filter((c) => c.paths.includes(file.path))
      .map((c) => ({ Change: c.changeId, Caller: "/root", Source: c.sourceKind })),
    counts: { Added: file.additions, Removed: file.deletions },
  }));
  const net = { Added: response.totalAdditions, Removed: response.totalDeletions };
  return {
    fileCount: files.length,
    revision,
    workspace: response.workspace ?? "",
    coverage: "full",
    callers: files.length ? [{ caller: "/root", count: files.length }] : [],
    counts: {
      Callers: files.length ? { "/root": net } : {},
      Net: files.length ? net : null,
    },
    files,
  };
}

function threadToAgent(thread: Thread, depth: number): Agent {
  const name = depth === 0 ? "/root" : thread.agentNickname || thread.id;
  const label = depth === 0 ? "main" : thread.agentNickname || thread.name || thread.id;
  return {
    name,
    label,
    aliases: [name, thread.id],
    thread: thread.id,
    role: thread.agentRole ?? undefined,
    depth,
    state: threadStatusToAgentState(thread.status),
    turns: thread.turns?.length || undefined,
    context: thread.contextTokens,
    window: thread.contextWindow ?? undefined,
  };
}

export class Stream {
  connection = $state<Connection>("connecting");
  session = $state.raw<Session | undefined>();
  input = $state.raw<Input | undefined>();
  status = $state.raw<Status>({});
  main = $state.raw<Feed>(empty);
  agents = $state.raw<Feed>(empty);
  histories = $state.raw<Record<string, Feed>>({});
  roster = $state.raw<Roster>({ total: 0, agents: [] });
  family = $state.raw<{ root: string; threads: string[] } | undefined>();
  review = $state.raw<SavedReview | null>(null);
  live = $state.raw<Preview[]>([]);
  activeTurnId = $state<string | undefined>();
  byName = $derived(new Map(this.roster.agents.map((agent) => [agent.name, agent])));
  identities = $derived.by(() => {
    const identities = new Map<string, AgentIdentity>();
    const remember = (identity: AgentIdentity) => {
      for (const alias of identity.aliases) identities.set(alias, identity);
    };
    for (const feed of [this.main, this.agents, ...Object.values(this.histories)])
      for (const seq of feed.order)
        for (const identity of feed.entries[seq]?.identities ?? []) remember(identity);
    for (const agent of this.roster.agents) {
      for (const identity of agent.identities ?? []) remember(identity);
      remember({ name: agent.name, label: agent.label, aliases: agent.aliases ?? [agent.name] });
    }
    return identities;
  });
  roles = $state.raw<string[]>([]);
  error = $state<string | undefined>();
  notice = $state<string | undefined>();
  gone = $state.raw<Gone | undefined>();
  jumps = $state(0);

  #id?: string;
  #machine?: Machine;
  #turns: Turn[] = [];
  #reviewRevision = 0;
  #stopNotifications?: () => void;
  #stopOpen?: () => void;
  #requests = new AbortController();

  get machine(): Machine | undefined {
    return this.#machine;
  }

  get threadId(): string | undefined {
    return this.#id;
  }

  connect(id: string, machine: Machine, preserveRoster = false): void {
    if (id === this.#id && machine === this.#machine && this.#stopNotifications) return;
    preserveRoster &&= machine === this.#machine;
    this.close();
    this.#id = id;
    this.#machine = machine;
    this.#turns = [];
    this.#requests = new AbortController();
    this.connection = machine.connection;
    this.session = this.input = this.gone = undefined;
    this.activeTurnId = undefined;
    this.status = {};
    this.main = this.agents = empty;
    this.histories = {};
    if (!preserveRoster) {
      this.roster = { total: 0, agents: [] };
      this.family = undefined;
    }
    this.review = null;
    this.live = [];
    this.error = this.notice = undefined;
    this.#stopNotifications = machine.onNotification((n) => this.#onNotification(n));
    this.#stopOpen = machine.onOpen(() => {
      this.connection = "open";
      this.error = undefined;
      void this.#hydrate(id, machine);
    });
    if (machine.connection === "open") {
      void this.#hydrate(id, machine);
    }
  }

  async refresh(): Promise<void> {
    if (!this.#id || !this.#machine) return;
    await this.#hydrate(this.#id, this.#machine);
  }

  async #hydrate(id: string, machine: Machine): Promise<void> {
    const signal = this.#requests.signal;
    try {
      let thread: Thread;
      let model: string | undefined;
      let effort: string | undefined;
      let serviceTier: string | undefined;
      try {
        const resumed = await machine.request<ThreadResumeResponse>(
          "thread/resume",
          { threadId: id },
          { signal },
        );
        thread = resumed.thread;
        model = resumed.model || thread.model || undefined;
        effort = resumed.reasoningEffort ?? thread.reasoningEffort ?? undefined;
        serviceTier = resumed.serviceTier ?? thread.serviceTier ?? undefined;
      } catch (err) {
        if (signal.aborted) return;
        if (err instanceof TransportError && (err.status === 404 || err.status === 410)) {
          this.gone = { status: err.status, text: err.message };
          return;
        }
        const read = await machine.request<ThreadReadResponse>(
          "thread/read",
          { threadId: id, includeTurns: true },
          { signal },
        );
        thread = read.thread;
        model = thread.model ?? undefined;
        effort = thread.reasoningEffort ?? undefined;
        serviceTier = thread.serviceTier ?? undefined;
      }
      if (signal.aborted) return;
      this.gone = undefined;
      this.connection = "open";
      this.error = undefined;
      this.#applyThread(thread, model, effort, serviceTier);
      void this.#loadSubtree(id, machine, signal);
      void this.#loadChanges(id, machine, signal);
    } catch (err) {
      if (signal.aborted) return;
      this.error = errorText(err);
      if (err instanceof TransportError && (err.status === 404 || err.status === 410)) {
        this.gone = { status: err.status, text: err.message };
      }
    }
  }

  #applyThread(thread: Thread, model?: string, effort?: string, serviceTier?: string): void {
    this.#turns = (thread.turns ?? []).map((t) => ({ ...t, items: [...t.items] }));
    let activeTurn: Turn | undefined;
    for (let i = this.#turns.length - 1; i >= 0; i--) {
      if (this.#turns[i].status === "inProgress") {
        activeTurn = this.#turns[i];
        break;
      }
    }
    this.activeTurnId = activeTurn?.id;
    const status = threadStatusToStatus(thread.status);
    if (activeTurn) status.running = true;
    this.status = status;
    this.input = { stoppable: !!activeTurn || !!status.running };
    this.session = {
      thread: thread.id,
      title: thread.name || thread.preview || undefined,
      workspace: thread.cwd || undefined,
      started: thread.createdAt ? thread.createdAt * 1000 : undefined,
      model: model ?? thread.model ?? this.session?.model,
      effort: effort ?? thread.reasoningEffort ?? this.session?.effort,
      serviceTier: serviceTier ?? thread.serviceTier ?? this.session?.serviceTier,
    };
    this.main = buildFeed(this.#turns, thread.id);
    if (!this.roster.agents.length) {
      this.roster = { total: 1, agents: [threadToAgent(thread, 0)] };
      this.#observeRoles();
    }
    this.jumps++;
  }

  async #loadSubtree(id: string, machine: Machine, signal: AbortSignal): Promise<void> {
    try {
      const response = await machine.request<ThreadListResponse>(
        "thread/subtree/list",
        { threadId: id },
        { signal },
      );
      if (signal.aborted) return;
      const threads = response.data ?? [];
      if (!threads.length) return;
      const root = threads.find((t) => !t.parentThreadId) ?? threads[0];
      this.family = { root: root.id, threads: threads.map((t) => t.id) };
      const agents = threads.map((t) => threadToAgent(t, t.id === root.id ? 0 : 1));
      this.roster = { total: agents.length, agents };
      this.#observeRoles();
    } catch {
      // Subtree listing is optional for single-agent threads.
    }
  }

  async #loadChanges(id: string, machine: Machine, signal: AbortSignal): Promise<void> {
    try {
      const response = await machine.request<ThreadChangesReadResponse>(
        "thread/changes/read",
        { threadId: id },
        { signal },
      );
      if (signal.aborted) return;
      this.review = changesToReview(response, ++this.#reviewRevision);
    } catch {
      // Change history may be unavailable for threads with no edits.
    }
  }

  #onNotification(notification: ServerNotification): void {
    const id = this.#id;
    const machine = this.#machine;
    if (!id || !machine) return;
    switch (notification.method) {
      case "thread/status/changed": {
        if (notification.params.threadId !== id) return;
        const status = threadStatusToStatus(notification.params.status);
        this.status = status;
        this.input = { ...this.input, stoppable: !!status.running || !!this.activeTurnId };
        this.roster = {
          ...this.roster,
          agents: this.roster.agents.map((a) =>
            a.thread === id
              ? { ...a, state: threadStatusToAgentState(notification.params.status) }
              : a,
          ),
        };
        break;
      }
      case "thread/name/updated": {
        if (notification.params.threadId !== id || !this.session) return;
        this.session = {
          ...this.session,
          title: notification.params.threadName || this.session.title,
        };
        break;
      }
      case "thread/settings/updated": {
        if (notification.params.threadId !== id || !this.session) return;
        const settings = notification.params.threadSettings;
        this.session = {
          ...this.session,
          model: settings.model || this.session.model,
          effort: settings.effort ?? undefined,
          serviceTier: settings.serviceTier ?? undefined,
        };
        break;
      }
      case "thread/tokenUsage/updated": {
        if (notification.params.threadId !== id) return;
        const usage = notification.params.tokenUsage;
        this.roster = {
          ...this.roster,
          agents: this.roster.agents.map((a) =>
            a.thread === id || a.name === "/root"
              ? {
                  ...a,
                  inputTokens: usage.total.inputTokens,
                  outputTokens: usage.total.outputTokens,
                  context: usage.last.totalTokens,
                  window: usage.modelContextWindow ?? a.window,
                }
              : a,
          ),
        };
        break;
      }
      case "turn/started": {
        if (notification.params.threadId !== id) return;
        const turn = notification.params.turn;
        this.activeTurnId = turn.id;
        this.status = {
          ...this.status,
          running: true,
          text: "Working",
          failure: undefined,
          alert: false,
        };
        this.input = { ...this.input, stoppable: true };
        this.#upsertTurn(turn);
        this.main = buildFeed(this.#turns, id);
        break;
      }
      case "turn/completed": {
        if (notification.params.threadId !== id) return;
        const turn = notification.params.turn;
        if (this.activeTurnId === turn.id) this.activeTurnId = undefined;
        const failed = turn.status === "failed";
        this.status = {
          running: false,
          alert: failed,
          failure: turn.error?.message,
          text: failed
            ? turn.error?.message
            : turn.status === "interrupted"
              ? "Interrupted"
              : undefined,
        };
        this.input = { ...this.input, stoppable: false };
        this.#upsertTurn(turn);
        this.main = buildFeed(this.#turns, id);
        void this.#loadChanges(id, machine, this.#requests.signal);
        break;
      }
      case "item/started":
      case "item/completed": {
        if (notification.params.threadId !== id) return;
        this.#upsertItem(notification.params.turnId, notification.params.item);
        this.main = buildFeed(this.#turns, id);
        break;
      }
      case "item/agentMessage/delta": {
        if (notification.params.threadId !== id) return;
        this.#appendItemDelta(notification.params.turnId, notification.params.itemId, (item) => {
          if (item.type === "agentMessage")
            return { ...item, text: item.text + notification.params.delta };
          return item;
        });
        this.main = buildFeed(this.#turns, id);
        break;
      }
      case "item/plan/delta": {
        if (notification.params.threadId !== id) return;
        this.#appendItemDelta(notification.params.turnId, notification.params.itemId, (item) => {
          if (item.type === "plan") return { ...item, text: item.text + notification.params.delta };
          return item;
        });
        this.main = buildFeed(this.#turns, id);
        break;
      }
      case "item/commandExecution/outputDelta": {
        if (notification.params.threadId !== id) return;
        this.#appendItemDelta(notification.params.turnId, notification.params.itemId, (item) => {
          if (item.type === "commandExecution")
            return {
              ...item,
              aggregatedOutput: (item.aggregatedOutput ?? "") + notification.params.delta,
            };
          return item;
        });
        this.main = buildFeed(this.#turns, id);
        break;
      }
      case "item/reasoning/summaryTextDelta": {
        if (notification.params.threadId !== id) return;
        this.#appendItemDelta(notification.params.turnId, notification.params.itemId, (item) => {
          if (item.type !== "reasoning") return item;
          const summary = [...item.summary];
          const idx = notification.params.summaryIndex;
          while (summary.length <= idx) summary.push("");
          summary[idx] += notification.params.delta;
          return { ...item, summary };
        });
        this.main = buildFeed(this.#turns, id);
        break;
      }
      case "item/reasoning/textDelta": {
        if (notification.params.threadId !== id) return;
        this.#appendItemDelta(notification.params.turnId, notification.params.itemId, (item) => {
          if (item.type !== "reasoning") return item;
          const content = [...item.content];
          const idx = notification.params.contentIndex;
          while (content.length <= idx) content.push("");
          content[idx] += notification.params.delta;
          return { ...item, content };
        });
        this.main = buildFeed(this.#turns, id);
        break;
      }
      case "turn/diff/updated": {
        if (notification.params.threadId !== id) return;
        void this.#loadChanges(id, machine, this.#requests.signal);
        break;
      }
      case "thread/reverted": {
        if (notification.params.threadId !== id) return;
        void this.#hydrate(id, machine);
        break;
      }
      default:
        break;
    }
  }

  #upsertTurn(turn: Turn): void {
    const index = this.#turns.findIndex((t) => t.id === turn.id);
    if (index >= 0) {
      const existing = this.#turns[index];
      this.#turns[index] = {
        ...turn,
        items: turn.items.length ? [...turn.items] : existing.items,
      };
    } else {
      this.#turns.push({ ...turn, items: [...turn.items] });
    }
  }

  #upsertItem(turnId: string, item: ThreadItem): void {
    let turn = this.#turns.find((t) => t.id === turnId);
    if (!turn) {
      const created: Turn = {
        id: turnId,
        rootTurnId: null,
        items: [],
        itemsView: "full",
        status: "inProgress",
        error: null,
        startedAt: null,
        completedAt: null,
        durationMs: null,
      };
      this.#turns.push(created);
      turn = created;
    }
    const index = turn.items.findIndex((existing) => existing.id === item.id);
    if (index >= 0) turn.items[index] = item;
    else turn.items.push(item);
  }

  #appendItemDelta(turnId: string, itemId: string, update: (item: ThreadItem) => ThreadItem): void {
    const turn = this.#turns.find((t) => t.id === turnId);
    if (!turn) return;
    const index = turn.items.findIndex((existing) => existing.id === itemId);
    if (index < 0) return;
    turn.items[index] = update(turn.items[index]);
  }

  close(): void {
    this.#stopNotifications?.();
    this.#stopNotifications = undefined;
    this.#stopOpen?.();
    this.#stopOpen = undefined;
    this.#requests.abort();
  }

  #observeRoles(): void {
    const seen = this.roles;
    const added = this.roster.agents
      .map((agent) => agent.role)
      .filter((role, i, all): role is string => !!role && all.indexOf(role) === i)
      .filter((role) => !seen.includes(role));
    if (added.length) this.roles = [...seen, ...added];
  }

  roleColor(role: string | undefined): string | undefined {
    const index = role ? this.roles.indexOf(role) : -1;
    return index < 0 ? undefined : roleColor(index);
  }

  label(name: string | undefined): string {
    if (!name || name === "/root") return "main";
    return this.identities.get(name)?.label ?? name;
  }

  agent(name: string | undefined): Agent | undefined {
    return name ? this.byName.get(this.address(name)) : undefined;
  }

  address(name: string): string {
    return this.identities.get(name)?.name ?? name;
  }

  async readContent(ref: ContentReference, signal?: AbortSignal): Promise<ContentResourceResponse> {
    const machine = this.#machine;
    const id = this.#id;
    if (!machine || !id) throw new Error("Open a thread on an execution machine first.");
    const lifetime = signal
      ? AbortSignal.any([signal, this.#requests.signal])
      : this.#requests.signal;
    return machine.resource<ContentResourceResponse>(
      "content",
      { thread: id, body: { thread: id, turnId: ref.turnId, itemId: ref.itemId } },
      { signal: lifetime },
    );
  }

  async image(
    entry: Pick<Entry, "itemId" | "seq" | "agent">,
    index: number,
    image: string,
    signal: AbortSignal,
  ): Promise<Blob> {
    const machine = this.#machine;
    const id = this.#id;
    if (!machine || !id) throw new Error("The image's machine is disconnected.");
    return machine.resource<Blob>(
      "image",
      { thread: id, body: { itemId: entry.itemId, index, image } },
      { signal: AbortSignal.any([signal, this.#requests.signal]) },
    );
  }
}

export const stream = new Stream();
