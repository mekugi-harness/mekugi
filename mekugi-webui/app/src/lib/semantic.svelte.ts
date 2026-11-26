import type { Machine } from "./hub.svelte";
import { errorText, type Connection } from "./transport";
import { stream } from "./stream.svelte";
import type { PendingAttachment } from "./attachments";
import type {
  Approval,
  Attachment,
  CommandExecutionRequestApprovalResponse,
  FileChangeRequestApprovalResponse,
  Prompts,
  Question,
  QuestionAnswer,
  QuestionCall,
  QueuedSubmission,
  ReasoningEffort,
  ServerNotification,
  ServerRequest,
  ShellGuardRequestApprovalResponse,
  ThreadForkResponse,
  ThreadQueueAddParams,
  ThreadQueueListResponse,
  ThreadRevertResponse,
  ThreadSetNameParams,
  ThreadSettingsUpdateParams,
  ToolRequestUserInputAnswer,
  ToolRequestUserInputResponse,
  TurnInterruptParams,
  TurnStartParams,
  TurnSteerParams,
  UserInput,
} from "./protocol";

export type Editor = {
  text: string;
  items: PendingAttachment[];
  revision: number;
  submission?: AbortController;
};

export type QuestionDraft = {
  choice: number | "skip" | undefined;
  text: string;
  note: string;
  sending: boolean;
  accepted: boolean;
};

export type PromptState = {
  drafts: Record<string, QuestionDraft>;
  deny?: number;
  reason: string;
  sending: boolean;
  accepted: boolean;
  error?: string;
  resolved?: boolean;
};

export function questionDraft(question: Question): QuestionDraft {
  const recommended = question.options?.findIndex((option) => option.recommended) ?? -1;
  return {
    choice: recommended >= 0 ? recommended : question.options?.length ? undefined : 0,
    text: "",
    note: "",
    sending: false,
    accepted: false,
  };
}

export function questionAnswers(
  call: QuestionCall,
  drafts: Record<string, QuestionDraft>,
  id?: string,
): QuestionAnswer[] {
  const questions = call.blocking ? call.questions : call.questions.filter((q) => q.id === id);
  if (!questions.length) throw new Error("Choose a question to answer.");
  return questions.map((q) => {
    const draft = drafts[q.id];
    const written = draft?.choice === (q.options?.length ?? 0);
    if (!draft || draft.choice === undefined || (written && !draft.text.trim())) {
      throw new Error(
        call.blocking
          ? "Answer every question or skip it."
          : "Choose an option, write an answer or skip this question.",
        { cause: q.id },
      );
    }
    return {
      question: q.id,
      choice: typeof draft.choice === "number" ? draft.choice : undefined,
      text: written ? draft.text : "",
      note: typeof draft.choice === "number" && !written ? draft.note : "",
      skip: draft.choice === "skip",
    };
  });
}

function inferImageMime(name: string): string {
  const ext = name.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "png":
      return "image/png";
    case "jpg":
    case "jpeg":
      return "image/jpeg";
    case "gif":
      return "image/gif";
    case "webp":
      return "image/webp";
    case "svg":
      return "image/svg+xml";
    case "bmp":
      return "image/bmp";
    case "avif":
      return "image/avif";
    default:
      return "application/octet-stream";
  }
}

function newClientMessageId(): string {
  return (
    globalThis.crypto?.randomUUID?.() ??
    `msg-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
  );
}

export function formatShellArg(arg: string): string {
  if (arg === "") return "''";
  if (/^[A-Za-z0-9_./:@%+=,-]+$/.test(arg)) return arg;
  for (let i = 0; i < arg.length; i++) {
    const code = arg.charCodeAt(i);
    if (code < 0x20 || code === 0x7f) return JSON.stringify(arg);
  }
  return `'${arg.replaceAll("'", "'\\''")}'`;
}

export function toNativeUserInput(text: string, attachments: Attachment[] = []): UserInput[] {
  const items: UserInput[] = [];
  if (text.trim()) {
    items.push({ type: "text", text, text_elements: [] });
  }
  for (const att of attachments) {
    if (att.source === "path") {
      items.push({ type: "localImage", path: att.path });
    } else if (att.source === "upload") {
      const url = att.data.startsWith("data:")
        ? att.data
        : `data:${att.mime || inferImageMime(att.name)};base64,${att.data}`;
      items.push({ type: "image", url });
    } else if (att.source === "selection") {
      items.push({ type: "text", text: att.data, text_elements: [] });
    }
  }
  return items;
}

// One-time-only choices: RemoteHuman connections are rejected by mekugi-app-server
// if they submit acceptForSession, policy amendments, or secret prompt answers.
const ONE_TIME_CHOICES = [
  { id: 0, label: "Allow once", decision: "accept" as const },
  { id: 1, label: "Deny", decision: "decline" as const, deny: true },
];

export function serverRequestToPrompt(
  req: ServerRequest,
  threadId: string,
): { kind: "approval"; approval: Approval } | { kind: "question"; call: QuestionCall } | undefined {
  switch (req.method) {
    case "item/commandExecution/requestApproval": {
      if (req.params.threadId !== threadId) return undefined;
      const details: string[] = [];
      if (req.params.cwd) details.push(`Directory: ${req.params.cwd}`);
      if (req.params.reason) details.push(req.params.reason);
      return {
        kind: "approval",
        approval: {
          id: String(req.id),
          requestId: req.id,
          method: req.method,
          threadId,
          title: "Approve command execution",
          subject: req.params.command ?? undefined,
          details,
          choices: [
            { id: 0, label: "Allow once", decision: "accept" },
            { id: 1, label: "Decline", decision: "decline" },
          ],
        },
      };
    }
    case "item/fileChange/requestApproval": {
      if (req.params.threadId !== threadId) return undefined;
      const details: string[] = [];
      if (req.params.reason) details.push(req.params.reason);
      return {
        kind: "approval",
        approval: {
          id: String(req.id),
          requestId: req.id,
          method: req.method,
          threadId,
          title: "Approve file change",
          subject: req.params.grantRoot ?? undefined,
          details,
          choices: [
            { id: 0, label: "Allow once", decision: "accept" },
            { id: 1, label: "Decline", decision: "decline" },
          ],
        },
      };
    }
    case "shellGuard/requestApproval": {
      if (req.params.threadId !== threadId) return undefined;
      const command = [req.params.executable, ...req.params.argv].map(formatShellArg).join(" ");
      return {
        kind: "approval",
        approval: {
          id: String(req.id),
          requestId: req.id,
          method: req.method,
          threadId,
          title: "Approve guarded shell action",
          subject: command,
          details: req.params.cwd ? [`Directory: ${req.params.cwd}`] : [],
          choices: ONE_TIME_CHOICES,
        },
      };
    }
    case "item/tool/requestUserInput": {
      if (req.params.threadId !== threadId) return undefined;
      const hasSecret = req.params.questions.some((q) => q.isSecret);
      return {
        kind: "question",
        call: {
          id: String(req.id),
          requestId: req.id,
          threadId,
          blocking: req.params.isBlocking,
          secretBlocked: hasSecret,
          questions: req.params.questions.map((q) => ({
            id: q.id,
            header: q.header ?? undefined,
            question: q.question,
            text: q.question,
            isOther: q.isOther,
            isSecret: q.isSecret,
            secret: q.isSecret,
            options: q.options?.map((o) => ({
              label: o.label,
              description: o.description ?? undefined,
              preview: o.preview ?? undefined,
              recommended: o.recommended,
            })),
          })),
        },
      };
    }
    default:
      // shellGuard/requestSecret and other local-only requests are never answered remotely.
      return undefined;
  }
}

export class ThreadControl {
  #id?: string;
  #machine?: Machine;
  #stopRequests?: () => void;
  #stopNotifications?: () => void;
  #editors = new Map<string, Editor>();
  #promptStates = new Map<string, PromptState>();

  pending = $state(0);
  error = $state<string | undefined>();
  prompts = $state.raw<Prompts>({ approvals: [], questions: [] });
  queued = $state.raw<QueuedSubmission[]>([]);

  get connection(): Connection {
    return this.#machine?.connection ?? "connecting";
  }

  get threadId(): string | undefined {
    return this.#id;
  }

  get machine(): Machine | undefined {
    return this.#machine;
  }

  connect(id: string, machine: Machine): void {
    if (this.#id === id && this.#machine === machine && this.#stopRequests) return;
    this.release();
    this.#id = id;
    this.#machine = machine;
    this.error = undefined;
    this.prompts = { approvals: [], questions: [] };
    this.queued = [];
    this.#promptStates.clear();
    this.#stopRequests = machine.onServerRequest((req) => this.#onServerRequest(req));
    this.#stopNotifications = machine.onNotification((n) => this.#onNotification(n));
    void this.#refreshQueue();
  }

  release(id?: string, machine?: Machine): void {
    if (id && this.#id !== id) return;
    if (machine && this.#machine !== machine) return;
    this.#stopRequests?.();
    this.#stopRequests = undefined;
    this.#stopNotifications?.();
    this.#stopNotifications = undefined;
    this.#id = undefined;
    this.#machine = undefined;
    this.prompts = { approvals: [], questions: [] };
    this.queued = [];
    this.#promptStates.clear();
    this.error = undefined;
  }

  editor(key = this.#id ?? "default"): Editor {
    let entry = this.#editors.get(key);
    if (!entry) {
      entry = { text: "", items: [], revision: 0 };
      this.#editors.set(key, entry);
    }
    return entry;
  }

  prompt(id: string): PromptState {
    let state = this.#promptStates.get(id);
    if (!state) {
      const created = $state<PromptState>({
        drafts: {},
        reason: "",
        sending: false,
        accepted: false,
      });
      this.#promptStates.set(id, created);
      state = created;
    }
    return state;
  }

  #onServerRequest(req: ServerRequest): void {
    if (!this.#id) return;
    const mapped = serverRequestToPrompt(req, this.#id);
    if (!mapped) return;
    if (mapped.kind === "approval") {
      if (this.prompts.approvals.some((a) => a.id === mapped.approval.id)) return;
      this.prompts = {
        ...this.prompts,
        approvals: [...this.prompts.approvals, mapped.approval],
      };
    } else {
      if (this.prompts.questions.some((q) => q.id === mapped.call.id)) return;
      this.prompts = {
        ...this.prompts,
        questions: [...this.prompts.questions, mapped.call],
      };
    }
  }

  #onNotification(notification: ServerNotification): void {
    if (!this.#id) return;
    if (notification.method === "serverRequest/resolved") {
      if (notification.params.threadId !== this.#id) return;
      const resolvedId = String(notification.params.requestId);
      this.prompts = {
        approvals: this.prompts.approvals.filter((a) => a.id !== resolvedId),
        questions: this.prompts.questions.filter((q) => q.id !== resolvedId),
      };
      this.#promptStates.delete(resolvedId);
      return;
    }
    if (notification.method === "thread/queue/changed") {
      if (notification.params.threadId !== this.#id) return;
      void this.#refreshQueue();
    }
  }

  async #refreshQueue(): Promise<void> {
    if (!this.#id || !this.#machine || this.#machine.connection !== "open") return;
    const threadId = this.#id;
    const machine = this.#machine;
    try {
      const all: QueuedSubmission[] = [];
      let cursor: string | undefined;
      for (let page = 0; page < 10; page++) {
        const response = await machine.request<ThreadQueueListResponse>("thread/queue/list", {
          threadId,
          ...(cursor ? { cursor } : {}),
        });
        if (this.#id !== threadId || this.#machine !== machine) return;
        for (const item of response.data ?? []) all.push(item);
        const next = response.nextCursor ?? undefined;
        if (!next || next === cursor) break;
        cursor = next;
      }
      this.queued = all;
    } catch {
      // Queue listing is optional.
    }
  }

  async sendInput(
    text: string,
    attachments: Attachment[] = [],
    options: { queue?: boolean; signal?: AbortSignal } = {},
  ): Promise<string | undefined> {
    const id = this.#id;
    const machine = this.#machine;
    if (!id || !machine) return "Open a thread on an execution machine first.";
    if (!attachments.length) {
      const slash = await this.#trySlashCommand(text, options.signal);
      if (slash.handled) return slash.error;
    }
    const input = toNativeUserInput(text, attachments);
    if (!input.length) return "Write a message or attach something.";
    this.pending++;
    this.error = undefined;
    try {
      if (options.queue) {
        const params: ThreadQueueAddParams = {
          threadId: id,
          input,
          clientUserMessageId: newClientMessageId(),
        };
        await machine.request("thread/queue/add", params, { signal: options.signal });
        void this.#refreshQueue();
      } else if (stream.activeTurnId) {
        const params: TurnSteerParams = {
          threadId: id,
          expectedTurnId: stream.activeTurnId,
          input,
        };
        await machine.request("turn/steer", params, { signal: options.signal });
      } else {
        const params: TurnStartParams = { threadId: id, input };
        await machine.request("turn/start", params, { signal: options.signal });
      }
      return undefined;
    } catch (err) {
      const message = errorText(err, true);
      this.error = message;
      return message;
    } finally {
      this.pending = Math.max(0, this.pending - 1);
    }
  }

  async #trySlashCommand(
    raw: string,
    signal?: AbortSignal,
  ): Promise<{ handled: boolean; error?: string }> {
    const trimmed = raw.trim();
    if (!trimmed.startsWith("/")) return { handled: false };
    const spaceIdx = trimmed.search(/\s/);
    const cmd = (spaceIdx >= 0 ? trimmed.slice(0, spaceIdx) : trimmed).toLowerCase();
    const rest = spaceIdx >= 0 ? trimmed.slice(spaceIdx + 1).trim() : "";
    if (cmd.indexOf("/", 1) >= 0) return { handled: false };
    const id = this.#id;
    const machine = this.#machine;
    if (!id || !machine)
      return { handled: true, error: "Open a thread on an execution machine first." };
    switch (cmd) {
      case "/compact":
        if (rest) return { handled: false };
        try {
          await machine.request("thread/compact/start", { threadId: id }, { signal });
          return { handled: true };
        } catch (err) {
          return { handled: true, error: errorText(err, true) };
        }
      case "/model":
        if (!rest) return { handled: true, error: "Specify a model name, e.g. /model gpt-5" };
        return { handled: true, error: await this.updateSettings({ model: rest }) };
      case "/effort":
      case "/reasoning":
        if (!rest)
          return { handled: true, error: "Specify reasoning effort (low, medium, high, xhigh)" };
        return {
          handled: true,
          error: await this.updateSettings({ effort: rest as ReasoningEffort }),
        };
      case "/tier": {
        if (!rest) return { handled: true, error: "Specify service tier (standard, fast)" };
        const lower = rest.toLowerCase();
        const tier = lower === "standard" || lower === "default" || lower === "none" ? null : rest;
        return { handled: true, error: await this.updateSettings({ serviceTier: tier }) };
      }
      case "/title":
        if (!rest) return { handled: true, error: "Specify a session title, e.g. /title My task" };
        return { handled: true, error: await this.rename(rest) };
      case "/archive":
        if (rest) return { handled: false };
        try {
          await machine.request("thread/archive", { threadId: id }, { signal });
          return { handled: true };
        } catch (err) {
          return { handled: true, error: errorText(err, true) };
        }
      default:
        return { handled: false };
    }
  }

  async interrupt(): Promise<string | undefined> {
    const id = this.#id;
    const machine = this.#machine;
    const turnId = stream.activeTurnId;
    if (!id || !machine || !turnId) return "No active turn to interrupt.";
    this.pending++;
    try {
      const params: TurnInterruptParams = { threadId: id, turnId };
      await machine.request("turn/interrupt", params);
      return undefined;
    } catch (err) {
      return errorText(err, true);
    } finally {
      this.pending = Math.max(0, this.pending - 1);
    }
  }

  async rename(name: string): Promise<string | undefined> {
    const id = this.#id;
    const machine = this.#machine;
    if (!id || !machine) return "Open a thread on an execution machine first.";
    this.pending++;
    try {
      const params: ThreadSetNameParams = { threadId: id, name };
      await machine.request("thread/name/set", params);
      if (stream.session?.thread === id) {
        stream.session = { ...stream.session, title: name };
      }
      return undefined;
    } catch (err) {
      return errorText(err, true);
    } finally {
      this.pending = Math.max(0, this.pending - 1);
    }
  }

  async updateSettings(patch: {
    model?: string | null;
    effort?: ReasoningEffort | null;
    serviceTier?: string | null;
  }): Promise<string | undefined> {
    const id = this.#id;
    const machine = this.#machine;
    if (!id || !machine) return "Open a thread on an execution machine first.";
    this.pending++;
    try {
      const params: ThreadSettingsUpdateParams = { threadId: id, ...patch };
      await machine.request("thread/settings/update", params);
      return undefined;
    } catch (err) {
      return errorText(err, true);
    } finally {
      this.pending = Math.max(0, this.pending - 1);
    }
  }

  async fork(lastTurnId?: string): Promise<{ id: string } | { error: string }> {
    const id = this.#id;
    const machine = this.#machine;
    if (!id || !machine) return { error: "Open a thread on an execution machine first." };
    this.pending++;
    try {
      const response = await machine.request<ThreadForkResponse>("thread/fork", {
        threadId: id,
        lastTurnId: lastTurnId ?? null,
      });
      return { id: response.thread.id };
    } catch (err) {
      return { error: errorText(err, true) };
    } finally {
      this.pending = Math.max(0, this.pending - 1);
    }
  }

  async revert(beforeTurnId: string, beforeItemId?: string): Promise<string | undefined> {
    const id = this.#id;
    const machine = this.#machine;
    if (!id || !machine) return "Open a thread on an execution machine first.";
    this.pending++;
    try {
      await machine.request<ThreadRevertResponse>("thread/revert", {
        threadId: id,
        beforeTurnId,
        beforeItemId: beforeItemId ?? null,
      });
      return undefined;
    } catch (err) {
      return errorText(err, true);
    } finally {
      this.pending = Math.max(0, this.pending - 1);
    }
  }

  decideApproval(approval: Approval, choiceIndex: number, reason = ""): string | undefined {
    const machine = this.#machine;
    if (!machine || machine.connection !== "open") {
      return "Wait for this machine to reconnect before answering.";
    }
    const choice = approval.choices.find((c) => c.id === choiceIndex);
    if (!choice || (choice.decision !== "accept" && choice.decision !== "decline")) {
      return "Only one-time allow or decline decisions are permitted remotely.";
    }
    if (approval.method === "item/commandExecution/requestApproval") {
      const response: CommandExecutionRequestApprovalResponse = { decision: choice.decision };
      machine.respond(approval.requestId, response);
    } else if (approval.method === "item/fileChange/requestApproval") {
      const response: FileChangeRequestApprovalResponse = { decision: choice.decision };
      machine.respond(approval.requestId, response);
    } else if (approval.method === "shellGuard/requestApproval") {
      const response: ShellGuardRequestApprovalResponse = {
        decision: choice.decision,
        reason: choice.decision === "decline" && reason.trim() ? reason.trim() : null,
      };
      machine.respond(approval.requestId, response);
    }
    this.prompts = {
      ...this.prompts,
      approvals: this.prompts.approvals.filter((a) => a.id !== approval.id),
    };
    this.#promptStates.delete(approval.id);
    return undefined;
  }

  answerQuestions(call: QuestionCall, answers: QuestionAnswer[]): string | undefined {
    const machine = this.#machine;
    if (!machine || machine.connection !== "open") {
      return "Wait for this machine to reconnect before answering.";
    }
    if (call.secretBlocked || call.questions.some((q) => q.isSecret)) {
      return "Secret prompts can only be answered locally on the execution machine.";
    }
    const payload: ToolRequestUserInputResponse = {
      answers: answers.map((answer): ToolRequestUserInputAnswer => ({
        id: answer.question,
        ...(answer.choice !== undefined ? { choice: answer.choice } : {}),
        ...(answer.note ? { note: answer.note } : {}),
        ...(answer.text ? { other: answer.text } : {}),
        ...(answer.skip ? { skipped: true } : {}),
      })),
    };
    machine.respond(call.requestId, payload);
    this.prompts = {
      ...this.prompts,
      questions: this.prompts.questions.filter((q) => q.id !== call.id),
    };
    this.#promptStates.delete(call.id);
    return undefined;
  }
}

export const semantic = new ThreadControl();
