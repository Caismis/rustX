/** One disposable page reader keyed by durable AgentId, across activations. */
import type { AppServerSession } from "./session.ts";
import type { RuntimeClientTranscriptPage } from "../protocol/app-server.ts";

export class SubagentTranscript {
  readonly #session: Pick<AppServerSession, "agentTranscriptPage">;
  #generation = 0;
  #reading = false;
  #closed = false;
  #older = false;
  readonly selected: string;
  #page: RuntimeClientTranscriptPage | undefined;
  #error: string | undefined;
  onChange?: () => void;

  constructor(session: Pick<AppServerSession, "agentTranscriptPage">, id: string) {
    this.#session = session;
    this.selected = id;
  }
  get page(): RuntimeClientTranscriptPage | undefined { return this.#page; }
  get error(): string | undefined { return this.#error; }
  newest(): Promise<void> {
    this.#older = false;
    this.#page = undefined;
    return this.#read(undefined);
  }
  older(): Promise<void> {
    const boundary = this.#page?.next_cursor;
    if (boundary == null) return Promise.resolve();
    this.#older = true;
    return this.#read(boundary);
  }
  refresh(): Promise<void> {
    if (this.#older || this.#reading || this.#closed) return Promise.resolve();
    return this.#read(undefined);
  }
  dispose(): void {
    this.#closed = true;
    this.#generation++;
    this.#page = undefined;
    this.onChange = undefined;
  }
  async #read(before: string | undefined): Promise<void> {
    if (this.#closed) return;
    const generation = ++this.#generation;
    const id = this.selected;
    this.#reading = true;
    this.#error = undefined;
    this.onChange?.();
    try {
      const page = await this.#session.agentTranscriptPage(id, before);
      if (generation !== this.#generation || this.#closed) return;
      if (page !== undefined) this.#page = page;
    } catch (error) {
      if (generation !== this.#generation || this.#closed) return;
      this.#page = undefined;
      this.#error = `Child history unavailable: ${error instanceof Error ? error.message : String(error)}`;
    } finally {
      if (generation === this.#generation && !this.#closed) {
        this.#reading = false;
        this.onChange?.();
      }
    }
  }
}
