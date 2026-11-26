import { roleColor } from "./agents";
import { threadStatusToStatus, type Machine } from "./hub.svelte";
import { errorText, TransportError, type Connection } from "./transport";
import type {
  Agent,
  AgentIdentity,
  AgentState,
  ContentReference,
  ContentResourceResponse,
  Entry,
  HookRunSummary,
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
  TurnsPage,
  Value,
} from "./protocol";

function formatJsonCode(value: unknown): string | undefined {
  if (value === undefined || value === null) return undefined;
  if (typeof value === "string") return value.trim() ? value : undefined;
  try {
    const serialized = JSON.stringify(value, null, 2);
    return serialized && serialized !== "{}" && serialized !== "[]" ? serialized : undefined;
  } catch {
    return undefined;
  }
}

function linesOf(text: string | undefined): string[] | undefined {
  if (!text) return undefined;
  const trimmed = text.replace(/\n$/, "");
  return trimmed ? trimmed.split("\n") : undefined;
}

function extractMcpTail(
  result: { content?: unknown[]; structuredContent?: unknown } | null | undefined,
  error: { message: string } | null | undefined,
): string[] | undefined {
  if (error?.message) return linesOf(error.message);
  if (!result) return undefined;
  const parts: string[] = [];
  for (const entry of result.content ?? []) {
    if (entry && typeof entry === "object" && "text" in entry && typeof entry.text === "string") {
      parts.push(entry.text);
    }
  }
  if (parts.length) return linesOf(parts.join("\n"));
  if (result.structuredContent !== undefined && result.structuredContent !== null) {
    return linesOf(formatJsonCode(result.structuredContent));
  }
  return undefined;
}

function extractDynamicToolTail(
  items:
    | Array<{ type: string; text?: string; imageUrl?: string; audioUrl?: string }>
    | null
    | undefined,
): string[] | undefined {
  if (!items?.length) return undefined;
  const parts: string[] = [];
  for (const item of items) {
    if (item.type === "inputText" && item.text) parts.push(item.text);
    else if (item.type === "inputImage" && item.imageUrl) parts.push(`[image] ${item.imageUrl}`);
    else if (item.type === "inputAudio" && item.audioUrl) parts.push(`[audio] ${item.audioUrl}`);
  }
  return parts.length ? linesOf(parts.join("\n")) : undefined;
}

export function unwrapShellCommand(raw: string): string {
  const trimmed = raw.trim();
  const match = /^(?:\/\S+\/)?(?:bash|sh|zsh|dash)\s+-(?:lc|c)\s+([\s\S]+)$/.exec(trimmed);
  if (!match) return raw;
  const arg = match[1].trim();
  if (arg.length >= 2 && arg.startsWith("'") && arg.endsWith("'")) {
    return arg.slice(1, -1).replace(/'\\''/g, "'");
  }
  if (arg.length >= 2 && arg.startsWith('"') && arg.endsWith('"')) {
    return arg
      .slice(1, -1)
      .replace(/\\(["\\$`])/g, "$1")
      .replace(/\\n/g, "\n");
  }
  if (!/\s/.test(arg)) return arg;
  return raw;
}

function countDiffLines(
  diff: string | undefined,
  kind: string,
): { added: number; removed: number } {
  if (!diff) return { added: 0, removed: 0 };
  let added = 0;
  let removed = 0;
  const lines = diff.replace(/\n$/, "").split("\n");
  const nonEmpty = lines.length > 0 && !(lines.length === 1 && lines[0] === "");
  const hasUnifiedHeader = lines.some(
    (line) => line.startsWith("+++") || line.startsWith("---") || line.startsWith("@@"),
  );
  if ((kind === "add" || kind === "delete") && !hasUnifiedHeader) {
    if (!nonEmpty) return { added: 0, removed: 0 };
    return kind === "add"
      ? { added: lines.length, removed: 0 }
      : { added: 0, removed: lines.length };
  }
  for (const line of lines) {
    if (line.startsWith("+++") || line.startsWith("---") || line.startsWith("@@")) {
      continue;
    }
    if (line.startsWith("+")) {
      added++;
    } else if (line.startsWith("-")) {
      removed++;
    }
  }
  return { added, removed };
}

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

export function itemToEntry(
  item: ThreadItem,
  seq: number,
  turnIndex: number,
  turnId: string,
  agent = "/root",
  live = false,
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
      const sections = [...(item.summary ?? []), ...(item.content ?? [])]
        .map((part) => part.trim())
        .filter(Boolean);
      const values: Value[] = sections.length
        ? sections.map((body, index) => {
            const heading = body.match(/^\*\*([^\n*]+)\*\*/)?.[1]?.trim();
            return {
              kind: "reasoning",
              label: heading || undefined,
              body,
              live: live && index === sections.length - 1,
            };
          })
        : live
          ? [{ kind: "reasoning", body: "", live: true }]
          : [];
      return {
        ...base,
        kind: "reasoning",
        running: live || undefined,
        values,
      };
    }
    case "commandExecution": {
      const running = item.status === "inProgress";
      const failed = item.status === "failed" || item.status === "declined";
      const tail = item.aggregatedOutput ? item.aggregatedOutput.split("\n") : undefined;
      const value: Value = {
        kind: "op",
        verb: "Run",
        code: unwrapShellCommand(item.command),
        lang: "bash",
        workdir: item.cwd || undefined,
        exitCode: item.exitCode ?? undefined,
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
      const values: Value[] = item.changes.length
        ? item.changes.map((c) => {
            const verb =
              c.kind.type === "add"
                ? "Create"
                : c.kind.type === "delete"
                  ? "Delete"
                  : c.kind.type === "update" && c.kind.move_path
                    ? "Move"
                    : "Edit";
            const path =
              c.kind.type === "update" && c.kind.move_path
                ? `${c.path} => ${c.kind.move_path}`
                : c.path;
            const { added, removed } = countDiffLines(c.diff, c.kind.type);
            const label = added || removed ? `+${added} −${removed}` : undefined;
            return {
              kind: "op",
              verb,
              path,
              label,
              code: c.diff || undefined,
              lang: c.diff ? "diff" : undefined,
              foldSource: c.diff ? true : undefined,
              running,
              failed,
            };
          })
        : [{ kind: "op", verb: "Edit", running, failed }];
      return {
        ...base,
        kind: "tool",
        running,
        outcome: failed ? "error" : undefined,
        values,
      };
    }
    case "mcpToolCall": {
      const running = item.status === "inProgress";
      const failed = item.status === "failed";
      const argsCode = formatJsonCode(item.arguments);
      const tail = extractMcpTail(item.result, item.error);
      const value: Value = {
        kind: "op",
        verb: "Call",
        label: `${item.server}:${item.tool}`,
        code: argsCode,
        lang: argsCode ? "json" : undefined,
        running,
        failed,
        duration: item.durationMs ?? undefined,
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
    case "dynamicToolCall": {
      const running = item.status === "inProgress";
      const failed = item.status === "failed" || item.success === false;
      const argsCode = formatJsonCode(item.arguments);
      const tail = extractDynamicToolTail(item.contentItems);
      const label = item.namespace ? `${item.namespace}:${item.tool}` : item.tool;
      const value: Value = {
        kind: "op",
        verb: "Call",
        label,
        code: argsCode,
        lang: argsCode ? "json" : undefined,
        running,
        failed,
        duration: item.durationMs ?? undefined,
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
        values: [{ kind: "read", verb: "View", path: item.path }],
      };
    case "subAgentActivity": {
      const target = item.agentPath;
      const body = item.assignment ?? "";
      return {
        ...base,
        kind: item.assignment ? "start" : "agent",
        assignment: !!item.assignment,
        assignmentTo: target,
        assignmentText: item.assignment ?? undefined,
        text: body || target,
        traffic: {
          kind: item.assignment ? "start" : "message",
          to: target,
          detail: item.assignment ? "assigned" : undefined,
        },
        values: [
          {
            kind: item.assignment ? "start" : "message",
            to: target,
            body,
          },
        ],
      };
    }
    case "collabAgentToolCall": {
      const target = item.receiverThreadIds?.[0];
      const failed = item.status === "failed";
      const body = item.prompt ?? "";
      const kind = failed
        ? "error"
        : item.tool === "spawnAgent" || item.tool === "resumeAgent"
          ? "start"
          : "message";
      return {
        ...base,
        kind: "agent",
        outcome: failed ? "error" : undefined,
        text: body || item.tool,
        traffic: {
          kind,
          from: agent,
          to: target,
          detail: item.tool,
        },
        values: [
          {
            kind: "message",
            from: agent,
            to: target,
            body,
          },
        ],
      };
    }
    case "contextCompaction":
      return {
        ...base,
        kind: "compaction",
        text: "Context compacted",
        values: [{ kind: "compaction", label: "Context compacted" }],
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

export interface HookRecord {
  key: string;
  id: string;
  turnId: string | null;
  afterItemCount: number;
  run: HookRunSummary;
}

export function hookRunToEntry(
  run: HookRunSummary,
  seq: number,
  turnId?: string,
  agent = "/root",
): Entry {
  const running = run.status === "running";
  const failed = run.status === "failed" || run.status === "blocked" || run.status === "stopped";
  const sourceName = run.sourcePath ? run.sourcePath.split("/").at(-1) : undefined;
  const labelParts = [
    run.eventName,
    sourceName ? `\`${sourceName}\`` : undefined,
    run.statusMessage || undefined,
  ].filter(Boolean);
  const tail = run.entries
    .flatMap((entry) => entry.text.split("\n"))
    .filter((line) => line.length > 0);
  if (failed && !tail.length && run.statusMessage) tail.push(run.statusMessage);
  const rawStarted = Number(run.startedAt);
  const observed =
    Number.isFinite(rawStarted) && rawStarted > 0
      ? rawStarted < 1e12
        ? rawStarted * 1000
        : rawStarted
      : undefined;
  return {
    seq,
    position: seq,
    itemId: `hook:${run.id}`,
    turnId,
    agent,
    observed,
    kind: "hook",
    running: running || undefined,
    outcome: failed ? "error" : undefined,
    values: [
      {
        kind: "hook",
        label: labelParts.join(" · "),
        duration: run.durationMs != null ? Number(run.durationMs) : undefined,
        running,
        failed,
        tail: tail.length ? tail : undefined,
        hook: {
          event: run.eventName,
          name: sourceName,
          message: run.statusMessage ?? undefined,
          status: run.status,
          hasError: failed,
          blocked: run.status === "blocked",
        },
      },
    ],
  };
}

export function buildFeed(
  turns: Turn[],
  attachment?: string,
  liveItemIds?: ReadonlySet<string>,
  hooks: readonly HookRecord[] = [],
): Feed {
  const entries: Record<number, Entry> = {};
  const order: number[] = [];
  let seq = 0;
  const emittedHooks = new Set<string>();
  const emitHook = (record: HookRecord) => {
    if (emittedHooks.has(record.key)) return;
    emittedHooks.add(record.key);
    seq++;
    entries[seq] = hookRunToEntry(record.run, seq, record.turnId ?? undefined);
    order.push(seq);
  };
  const hooksByTurn = new Map<string, HookRecord[]>();
  for (const hook of hooks) {
    if (!hook.turnId) continue;
    const list = hooksByTurn.get(hook.turnId);
    if (list) list.push(hook);
    else hooksByTurn.set(hook.turnId, [hook]);
  }
  for (const list of hooksByTurn.values()) {
    if (list.length > 1) list.sort((a, b) => a.afterItemCount - b.afterItemCount);
  }
  for (let t = 0; t < turns.length; t++) {
    const turn = turns[t];
    const turnHooks = hooksByTurn.get(turn.id) ?? [];
    let hookIdx = 0;
    for (let i = 0; i < turn.items.length; i++) {
      while (hookIdx < turnHooks.length && turnHooks[hookIdx].afterItemCount <= i) {
        emitHook(turnHooks[hookIdx++]);
      }
      const item = turn.items[i];
      seq++;
      entries[seq] = itemToEntry(item, seq, t + 1, turn.id, "/root", !!liveItemIds?.has(item.id));
      order.push(seq);
    }
    while (hookIdx < turnHooks.length) emitHook(turnHooks[hookIdx++]);
  }
  for (const hook of hooks) emitHook(hook);
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

const maxCachedHookThreads = 20;
const maxRoutineHooksPerThread = 200;
const maxFailedHooksPerThread = 200;

function isHookFailed(run: HookRunSummary): boolean {
  return run.status === "failed" || run.status === "blocked" || run.status === "stopped";
}

function trimHooks(hooks: HookRecord[]): HookRecord[] {
  let routine = 0;
  let failed = 0;
  for (const h of hooks) {
    if (isHookFailed(h.run)) failed++;
    else if (h.run.status !== "running") routine++;
  }
  if (routine <= maxRoutineHooksPerThread && failed <= maxFailedHooksPerThread) return hooks;
  let dropRoutine = Math.max(0, routine - maxRoutineHooksPerThread);
  let dropFailed = Math.max(0, failed - maxFailedHooksPerThread);
  return hooks.filter((h) => {
    if (h.run.status === "running") return true;
    if (isHookFailed(h.run)) {
      if (dropFailed > 0) {
        dropFailed--;
        return false;
      }
      return true;
    }
    if (dropRoutine > 0) {
      dropRoutine--;
      return false;
    }
    return true;
  });
}

export class Stream {
  session = $state.raw<Session | undefined>();
  input = $state.raw<Input | undefined>();
  status = $state.raw<Status>({});
  main = $state.raw<Feed>(empty);
  roster = $state.raw<Roster>({ total: 0, agents: [] });
  family = $state.raw<{ root: string; threads: string[] } | undefined>();
  review = $state.raw<SavedReview | null>(null);
  live = $state.raw<Preview[]>([]);
  activeTurnId = $state<string | undefined>();
  hasEarlier = $state(false);
  loadingEarlier = $state(false);
  byName = $derived(new Map(this.roster.agents.map((agent) => [agent.name, agent])));
  identities = $derived.by(() => {
    const identities = new Map<string, AgentIdentity>();
    const remember = (identity: AgentIdentity) => {
      for (const alias of identity.aliases) identities.set(alias, identity);
    };
    for (const seq of this.main.order)
      for (const identity of this.main.entries[seq]?.identities ?? []) remember(identity);
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
  #machine = $state<Machine | undefined>();
  #turns: Turn[] = [];
  #turnsBackwardsCursor?: string;
  #liveItemIds = new Set<string>();
  #hooks: HookRecord[] = [];
  #hooksByThread = new Map<string, HookRecord[]>();
  #hookSeq = 0;
  #reviewRevision = 0;
  #stopNotifications?: () => void;
  #stopOpen?: () => void;
  #requests = new AbortController();
  #turnStartedAtByThread = new Map<string, number>();

  get connection(): Connection {
    return this.#machine?.connection ?? "connecting";
  }

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
    this.#turnsBackwardsCursor = undefined;
    this.hasEarlier = false;
    this.loadingEarlier = false;
    this.#liveItemIds.clear();
    this.#turnStartedAtByThread.clear();
    const cachedHooks = this.#hooksByThread.get(`${machine.id}:${id}`);
    // Only restore settled hooks across a subscription gap; running hooks may have
    // finished while the thread was unobserved.
    this.#hooks = cachedHooks ? cachedHooks.filter((h) => h.run.status !== "running") : [];
    if (cachedHooks) this.#rememberHooks(`${machine.id}:${id}`, this.#hooks);
    this.#requests = new AbortController();
    this.session = this.input = this.gone = undefined;
    this.activeTurnId = undefined;
    this.status = {};
    this.main = empty;
    if (!preserveRoster) {
      this.roster = { total: 0, agents: [] };
      this.family = undefined;
    }
    this.review = null;
    this.live = [];
    this.error = this.notice = undefined;
    this.#stopNotifications = machine.onNotification((n) => this.#onNotification(n));
    this.#stopOpen = machine.onOpen(() => {
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

  async earlier(): Promise<boolean> {
    const id = this.#id;
    const machine = this.#machine;
    const cursor = this.#turnsBackwardsCursor;
    if (!id || !machine || !cursor || this.loadingEarlier) return false;
    const signal = this.#requests.signal;
    this.loadingEarlier = true;
    try {
      const page = await machine.request<TurnsPage>(
        "thread/turns/list",
        { threadId: id, cursor, sortDirection: "desc", limit: 20, itemsView: "full" },
        { signal },
      );
      if (signal.aborted || this.#id !== id || this.#machine !== machine) return false;
      const nextCursor = page.backwardsCursor ?? page.nextCursor ?? undefined;
      this.#turnsBackwardsCursor = nextCursor && nextCursor !== cursor ? nextCursor : undefined;
      this.hasEarlier = !!this.#turnsBackwardsCursor;
      const existingIds = new Set(this.#turns.map((t) => t.id));
      const older: Turn[] = [];
      for (let i = (page.data?.length ?? 0) - 1; i >= 0; i--) {
        const t = page.data[i];
        if (!existingIds.has(t.id)) older.push({ ...t, items: [...t.items] });
      }
      if (!older.length) return false;
      this.#turns = [...older, ...this.#turns];
      this.#rebuildMain();
      return true;
    } catch {
      return false;
    } finally {
      if (this.#id === id && this.#machine === machine) this.loadingEarlier = false;
    }
  }

  async #hydrate(id: string, machine: Machine): Promise<void> {
    const signal = this.#requests.signal;
    try {
      let thread: Thread;
      let model: string | undefined;
      let effort: string | undefined;
      let serviceTier: string | undefined;
      let turnsBackwardsCursor: string | undefined;
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
        turnsBackwardsCursor = resumed.turnsBackwardsCursor ?? undefined;
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
      this.error = undefined;
      this.#turnsBackwardsCursor = turnsBackwardsCursor;
      this.hasEarlier = !!turnsBackwardsCursor;
      this.#applyThread(thread, model, effort, serviceTier);
      const subtreeRoot = this.family?.threads.includes(id)
        ? this.family.root
        : (thread.parentThreadId ?? id);
      void this.#loadSubtree(subtreeRoot, machine, signal);
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
    this.#liveItemIds.clear();
    if (this.#hooks.some((h) => h.run.status === "running")) {
      this.#hooks = this.#hooks.filter((h) => h.run.status !== "running");
      if (this.#machine && this.#id) {
        this.#rememberHooks(`${this.#machine.id}:${this.#id}`, this.#hooks);
      }
    }
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
    this.#rebuildMain();
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
      const prevByThread = new Map(
        this.roster.agents.filter((a) => a.thread).map((a) => [a.thread!, a]),
      );
      const agents = threads.map((t) => {
        const next = threadToAgent(t, t.id === root.id ? 0 : 1);
        const prev = prevByThread.get(t.id);
        if (!prev) return next;
        return {
          ...next,
          inputTokens: prev.inputTokens ?? next.inputTokens,
          outputTokens: prev.outputTokens ?? next.outputTokens,
          cost: prev.cost ?? next.cost,
          costPartial: prev.costPartial ?? next.costPartial,
          tps: prev.tps ?? next.tps,
          work: prev.work ?? next.work,
          workSince: prev.workSince ?? next.workSince,
          roundtrips: prev.roundtrips ?? next.roundtrips,
          activity: prev.activity ?? next.activity,
          context: next.context ?? prev.context,
          window: next.window ?? prev.window,
        };
      });
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
      case "thread/started": {
        const started = notification.params.thread;
        const parent = started.parentThreadId;
        if (
          parent &&
          (parent === id || parent === this.family?.root || this.family?.threads.includes(parent))
        ) {
          const rootId = this.family?.root ?? parent;
          const threads = this.family?.threads ?? [rootId];
          if (!threads.includes(started.id)) {
            this.family = { root: rootId, threads: [...threads, started.id] };
          }
          const nextAgent = threadToAgent(started, 1);
          const existingIdx = this.roster.agents.findIndex((a) => a.thread === started.id);
          const agents =
            existingIdx >= 0
              ? this.roster.agents.map((a, idx) => (idx === existingIdx ? nextAgent : a))
              : [...this.roster.agents, nextAgent];
          this.roster = { total: agents.length, agents };
          this.#observeRoles();
        }
        break;
      }
      case "thread/status/changed": {
        const targetId = notification.params.threadId;
        if (targetId === id) {
          const status = threadStatusToStatus(notification.params.status);
          this.status = status;
          this.input = { ...this.input, stoppable: !!status.running || !!this.activeTurnId };
        }
        if (this.roster.agents.some((a) => a.thread === targetId)) {
          this.roster = {
            ...this.roster,
            agents: this.roster.agents.map((a) =>
              a.thread === targetId
                ? { ...a, state: threadStatusToAgentState(notification.params.status) }
                : a,
            ),
          };
        }
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
        const targetId = notification.params.threadId;
        const matchesMain = targetId === id;
        const matchesRoster = this.roster.agents.some((a) => a.thread === targetId);
        if (!matchesMain && !matchesRoster) return;
        const usage = notification.params.tokenUsage;
        this.roster = {
          ...this.roster,
          agents: this.roster.agents.map((a) =>
            a.thread === targetId || (matchesMain && !a.thread && a.name === "/root")
              ? {
                  ...a,
                  inputTokens: usage.total.inputTokens,
                  outputTokens: usage.total.outputTokens,
                  cost: usage.estimatedCostUsd ?? a.cost,
                  costPartial: usage.costPartial ?? a.costPartial,
                  tps: usage.outputTokensPerSecond ?? a.tps,
                  work: usage.workDurationMs ?? a.work,
                  roundtrips: usage.roundtrips ?? a.roundtrips,
                  context: usage.last.totalTokens,
                  window: usage.modelContextWindow ?? a.window,
                }
              : a,
          ),
        };
        break;
      }
      case "turn/started": {
        const targetId = notification.params.threadId;
        const matchesMain = targetId === id;
        const matchesRoster = this.roster.agents.some((a) => a.thread === targetId);
        if (!matchesMain && !matchesRoster) return;
        const turn = notification.params.turn;
        const startedAt = Date.now();
        this.#turnStartedAtByThread.set(targetId, startedAt);
        if (matchesMain) {
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
          this.#rebuildMain();
        }
        this.roster = {
          ...this.roster,
          agents: this.roster.agents.map((a) =>
            a.thread === targetId || (matchesMain && !a.thread && a.name === "/root")
              ? {
                  ...a,
                  state: "working",
                  workSince: startedAt,
                  turns: matchesMain ? this.#turns.length : (a.turns ?? 0) + 1,
                }
              : a,
          ),
        };
        break;
      }
      case "turn/completed": {
        const targetId = notification.params.threadId;
        const matchesMain = targetId === id;
        const matchesRoster = this.roster.agents.some((a) => a.thread === targetId);
        if (!matchesMain && !matchesRoster) return;
        const turn = notification.params.turn;
        const startedAt = this.#turnStartedAtByThread.get(targetId);
        this.#turnStartedAtByThread.delete(targetId);
        const elapsed =
          turn.durationMs != null
            ? Math.max(0, Number(turn.durationMs))
            : startedAt != null
              ? Math.max(0, Date.now() - startedAt)
              : undefined;
        const failed = turn.status === "failed";
        if (matchesMain) {
          if (this.activeTurnId === turn.id) this.activeTurnId = undefined;
          this.status = {
            running: false,
            alert: failed,
            completed: elapsed,
            failure: turn.error?.message,
            text: failed
              ? turn.error?.message
              : turn.status === "interrupted"
                ? "Interrupted"
                : undefined,
          };
          this.input = { ...this.input, stoppable: false };
          this.#upsertTurn(turn);
          this.#liveItemIds.clear();
          if (this.#hooks.some((h) => h.run.status === "running" && h.turnId === turn.id)) {
            this.#hooks = this.#hooks.filter(
              (h) => !(h.run.status === "running" && h.turnId === turn.id),
            );
            this.#rememberHooks(`${machine.id}:${id}`, this.#hooks);
          }
          this.#rebuildMain();
          void this.#loadChanges(id, machine, this.#requests.signal);
        }
        this.roster = {
          ...this.roster,
          agents: this.roster.agents.map((a) => {
            if (a.thread !== targetId && !(matchesMain && !a.thread && a.name === "/root"))
              return a;
            const work = a.work != null ? a.work : elapsed != null ? elapsed : undefined;
            return {
              ...a,
              state: failed ? "error" : "idle",
              work,
              workSince: undefined,
              lastResponse: Date.now(),
              activity: undefined,
              turns: matchesMain ? this.#turns.length : a.turns,
            };
          }),
        };
        break;
      }
      case "hook/started":
      case "hook/completed": {
        if (notification.params.threadId !== id) return;
        this.#upsertHook(
          notification.method === "hook/started",
          notification.params.turnId ?? null,
          notification.params.run,
        );
        this.#rebuildMain();
        break;
      }
      case "item/started":
      case "item/completed": {
        const targetId = notification.params.threadId;
        const started = notification.method === "item/started";
        if (targetId !== id) {
          const agent = this.roster.agents.find((a) => a.thread === targetId);
          if (agent) {
            const activity = itemToEntry(
              notification.params.item,
              0,
              0,
              notification.params.turnId,
              agent.name,
              started,
            );
            this.roster = {
              ...this.roster,
              agents: this.roster.agents.map((a) =>
                a.thread === targetId ? { ...a, activity } : a,
              ),
            };
          }
          return;
        }
        if (started) {
          this.#liveItemIds.add(notification.params.item.id);
        } else {
          this.#liveItemIds.delete(notification.params.item.id);
        }
        this.#upsertItem(notification.params.turnId, notification.params.item);
        this.#rebuildMain();
        const latestActivity = itemToEntry(
          notification.params.item,
          0,
          0,
          notification.params.turnId,
          "/root",
          started,
        );
        if (this.roster.agents.some((a) => a.thread === id || a.name === "/root")) {
          this.roster = {
            ...this.roster,
            agents: this.roster.agents.map((a) =>
              a.thread === id || (!a.thread && a.name === "/root")
                ? { ...a, activity: latestActivity }
                : a,
            ),
          };
        }
        if (
          notification.params.item.type === "collabAgentToolCall" &&
          (notification.params.item.tool === "spawnAgent" ||
            notification.params.item.tool === "resumeAgent" ||
            notification.params.item.tool === "closeAgent")
        ) {
          void this.#loadSubtree(this.family?.root ?? id, machine, this.#requests.signal);
        }
        break;
      }
      case "item/agentMessage/delta": {
        if (notification.params.threadId !== id) return;
        this.#appendItemDelta(notification.params.turnId, notification.params.itemId, (item) => {
          if (item.type === "agentMessage")
            return { ...item, text: item.text + notification.params.delta };
          return item;
        });
        this.#rebuildMain();
        break;
      }
      case "item/plan/delta": {
        if (notification.params.threadId !== id) return;
        this.#appendItemDelta(notification.params.turnId, notification.params.itemId, (item) => {
          if (item.type === "plan") return { ...item, text: item.text + notification.params.delta };
          return item;
        });
        this.#rebuildMain();
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
        this.#rebuildMain();
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
        this.#rebuildMain();
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
        this.#rebuildMain();
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

  #rebuildMain(): void {
    this.main = buildFeed(this.#turns, this.#id, this.#liveItemIds, this.#hooks);
  }

  #upsertHook(started: boolean, turnId: string | null, run: HookRunSummary): void {
    let matched = false;
    if (!started) {
      for (let i = this.#hooks.length - 1; i >= 0; i--) {
        const candidate = this.#hooks[i];
        if (
          candidate.id === run.id &&
          candidate.run.status === "running" &&
          (!turnId || !candidate.turnId || candidate.turnId === turnId)
        ) {
          candidate.run = run;
          if (turnId) candidate.turnId = turnId;
          matched = true;
          break;
        }
      }
    }
    if (!matched) {
      const turn = turnId ? this.#turns.find((t) => t.id === turnId) : undefined;
      this.#hooks.push({
        key: `${run.id}:${++this.#hookSeq}`,
        id: run.id,
        turnId,
        afterItemCount: turn ? turn.items.length : 0,
        run,
      });
    }
    this.#hooks = trimHooks(this.#hooks);
    if (this.#machine && this.#id) {
      this.#rememberHooks(`${this.#machine.id}:${this.#id}`, this.#hooks);
    }
  }

  #rememberHooks(cacheKey: string, hooks: HookRecord[]): void {
    this.#hooksByThread.delete(cacheKey);
    if (!hooks.length) return;
    this.#hooksByThread.set(cacheKey, hooks);
    while (this.#hooksByThread.size > maxCachedHookThreads) {
      const oldest = this.#hooksByThread.keys().next().value;
      if (oldest === undefined) break;
      this.#hooksByThread.delete(oldest);
    }
  }

  #upsertTurn(turn: Turn): void {
    const index = this.#turns.findIndex((t) => t.id === turn.id);
    if (index >= 0) {
      const existing = this.#turns[index];
      let items: ThreadItem[];
      if (turn.itemsView === "full" && turn.items.length) {
        items = [...turn.items];
      } else if (turn.items.length) {
        items = [...existing.items];
        for (const item of turn.items) {
          const itemIdx = items.findIndex((e) => e.id === item.id);
          if (itemIdx >= 0) items[itemIdx] = item;
          else items.push(item);
        }
      } else {
        items = existing.items;
      }
      this.#turns[index] = {
        ...turn,
        items,
        itemsView: existing.itemsView === "full" ? "full" : turn.itemsView,
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
    const prevId = this.#id;
    const prevMachine = this.#machine;
    this.#id = undefined;
    this.#machine = undefined;
    this.#liveItemIds.clear();
    this.#hooks = [];
    this.#stopNotifications?.();
    this.#stopNotifications = undefined;
    this.#stopOpen?.();
    this.#stopOpen = undefined;
    this.#requests.abort();
    if (prevId && prevMachine && prevMachine.connection === "open") {
      void prevMachine.request("thread/unsubscribe", { threadId: prevId }).catch(() => {});
    }
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
    // http_resources.rs matches body.image against SHA-256 hex digest or "".
    // When itemId is present, omit raw file paths/URLs so lookup matches by itemId + index.
    const identity = /^[0-9a-f]{64}$/i.test(image) ? image : "";
    return machine.resource<Blob>(
      "image",
      { thread: id, body: { itemId: entry.itemId, index, image: identity } },
      { signal: AbortSignal.any([signal, this.#requests.signal]) },
    );
  }
}

export const stream = new Stream();
