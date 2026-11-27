import type { Machine } from "./hub.svelte";
import type {
  ServerNotification,
  ThreadForkResponse,
  TurnStartResponse,
  UserInput,
} from "./protocol";
import { errorText } from "./transport";

type Exchange = {
  question: string;
  messages: Map<string, string>;
};

// One private answer-only fork of Main, shared by follow-up questions in the dock.
export class SideAnswer {
  exchanges = $state.raw<Exchange[]>([]);
  busy = $state(false);
  closing = $state(false);
  error = $state<string>();
  #fork?: Promise<ThreadForkResponse>;
  #thread?: string;
  #turn?: string;
  #listen?: () => void;
  #main: string;
  #machine: Machine;

  constructor(machine: Machine, main: string) {
    this.#machine = machine;
    this.#main = main;
  }

  async ask(question: string, input: UserInput[]): Promise<string | undefined> {
    if (this.busy || this.closing) return "Wait for the current side answer or close it.";
    this.busy = true;
    this.error = undefined;
    this.#turn = undefined;
    const previous = this.exchanges;
    this.exchanges = [...previous, { question, messages: new Map() }];
    try {
      if (!this.#thread) {
        this.#fork = this.#machine.request<ThreadForkResponse>("thread/fork", {
          threadId: this.#main,
          threadSource: "mekugi_btw",
          excludeTurns: true,
          sandbox: "read-only",
          approvalPolicy: "never",
        });
        const { thread } = await this.#fork;
        this.#thread = thread.id;
        if (this.closing) return undefined;
      }
      this.#listen = this.#machine.onNotification((event) => this.#observe(event));
      const { turn } = await this.#machine.request<TurnStartResponse>("turn/start", {
        threadId: this.#thread,
        input,
        environments: [],
      });
      if (this.busy) this.#turn = turn.id;
    } catch (error) {
      this.exchanges = previous;
      this.#settle();
      return errorText(error, true);
    } finally {
      this.#fork = undefined;
    }
    return undefined;
  }

  async close(): Promise<void> {
    if (this.closing) return;
    this.closing = true;
    try {
      const threadId = this.#thread ?? (await this.#fork)?.thread.id;
      if (!threadId) return;
      await this.#machine.request("thread/archive", { threadId });
      this.#thread = undefined;
      this.#settle();
      this.exchanges = [];
      this.error = undefined;
    } catch (error) {
      if (this.busy && !this.#turn && !this.#listen) this.#settle();
      this.error = errorText(error, true);
    } finally {
      this.closing = false;
    }
  }

  #settle() {
    this.#listen?.();
    this.#listen = undefined;
    this.#turn = undefined;
    this.busy = false;
  }

  #observe(event: ServerNotification) {
    switch (event.method) {
      case "turn/started":
        if (event.params.threadId === this.#thread) this.#turn = event.params.turn.id;
        break;
      case "item/agentMessage/delta":
      case "item/completed": {
        const { params } = event;
        if (params.threadId !== this.#thread || params.turnId !== this.#turn) return;
        const current = this.exchanges.at(-1);
        if (!current) return;
        const messages = new Map(current.messages);
        if (event.method === "item/agentMessage/delta") {
          const { itemId, delta } = event.params;
          messages.set(itemId, (messages.get(itemId) ?? "") + delta);
        } else {
          const { item } = event.params;
          if (item.type !== "agentMessage") return;
          messages.set(item.id, item.text);
        }
        this.exchanges = [...this.exchanges.slice(0, -1), { ...current, messages }];
        break;
      }
      case "turn/completed": {
        const { threadId, turn } = event.params;
        if (threadId !== this.#thread || turn.id !== this.#turn) return;
        this.error =
          turn.status === "failed" ? (turn.error?.message ?? "Side answer failed.") : undefined;
        this.#settle();
        break;
      }
    }
  }
}
