/** Attachment-owned observation of submitted native Session management requests. */
import type { RuntimeClientAttachment } from "../runtime/attachment.ts";
import type { SessionDeletePreview, SessionDeleteResult, SessionSummaryView } from "../protocol/types.ts";

export type DeletionClient = Pick<RuntimeClientAttachment, "listSessions" | "previewSessionDeletion" | "deleteSession" | "recoverSessionDeletion">;
export interface DeletionContext { query: string; ids: string[]; index: number; loaded: number }
export interface ReconciledSessions { sessions: SessionSummaryView[]; nextOffset?: number }
export type DeletionOutcome = SessionDeleteResult | { status: "unknown" };
type State =
  | { kind: "idle" }
  | { kind: "pending"; operation: "execute" | "recover"; sessionId: string }
  | { kind: "result"; outcome: DeletionOutcome; sessionId: string };

export class SessionDeletionWorkflow {
  readonly #client: DeletionClient;
  readonly #live: () => boolean;
  readonly #feedback: (text: string) => void;
  readonly #listeners = new Set<() => void>();
  #state: State = { kind: "idle" };
  #context: DeletionContext = { query: "", ids: [], index: 0, loaded: 0 };
  #generation = 0;
  #page: ReconciledSessions | undefined;
  #attention = false;
  #terminated = false;

  constructor(client: DeletionClient, live: () => boolean, feedback: (text: string) => void) {
    this.#client = client;
    this.#live = () => !this.#terminated && live();
    this.#feedback = feedback;
  }
  terminate(): void { this.#terminated = true; this.#listeners.clear(); }
  get state(): State { return this.#state; }
  get context(): DeletionContext { return this.#context; }
  get generation(): number { return this.#generation; }
  get page(): ReconciledSessions | undefined { return this.#page; }
  get needsPresentation(): boolean { return this.#live() && this.#attention; }
  subscribe(listener: () => void): () => void {
    this.#listeners.add(listener);
    return () => { this.#listeners.delete(listener); };
  }
  #publish(): void { for (const listener of this.#listeners) listener(); }

  /** Synchronous single-submit frontier; no popup or presentation lease survives here. */
  execute(preview: Readonly<SessionDeletePreview>, context: DeletionContext): void {
    if (!this.#live() || this.#state.kind === "pending" || this.canRecover()) return;
    this.#context = { ...context, ids: [...context.ids] };
    void this.#submit({ operation: "execute", sessionId: preview.session_id, revision: preview.target_revision });
  }
  recover(): void {
    const state = this.#state;
    if (!this.#live() || state.kind !== "result" || !this.canRecover()) return;
    void this.#submit({ operation: "recover", sessionId: state.sessionId });
  }
  canRecover(): boolean {
    return this.#state.kind === "result" && ["committed_cleanup_pending", "committed_durability_uncertain", "unknown"].includes(this.#state.outcome.status);
  }
  /** Closing a notice hides it, but cannot discard a native recovery capability. */
  dismiss(): void {
    if (this.#state.kind === "pending") return;
    this.#attention = false;
    if (!this.canRecover()) this.#state = { kind: "idle" };
  }
  /** A stale result hands only a new preview obligation back to presentation. */
  takeStale(): string | undefined {
    if (this.#state.kind !== "result" || this.#state.outcome.status !== "stale") return;
    const id = this.#state.sessionId;
    this.#state = { kind: "idle" };
    this.#attention = false;
    return id;
  }
  async #submit(request: { operation: "execute"; sessionId: string; revision: string } | { operation: "recover"; sessionId: string }): Promise<void> {
    const { operation, sessionId } = request;
    this.#state = { kind: "pending", operation, sessionId };
    this.#attention = true;
    // The owner is already installed before invoking the typed native boundary.
    this.#publish();
    let outcome: DeletionOutcome;
    try {
      outcome = request.operation === "execute"
        ? await this.#client.deleteSession(sessionId, request.revision)
        : await this.#client.recoverSessionDeletion(sessionId);
    } catch { outcome = { status: "unknown" }; }
    // Only concrete attachment/transport termination ends observation. A popup
    // replacement or a same-attachment snapshot is deliberately irrelevant.
    if (!this.#live()) return;
    await this.#reconcile();
    if (!this.#live()) return;
    this.#state = { kind: "result", outcome, sessionId };
    this.#attention = outcome.status !== "deleted";
    if (outcome.status === "deleted") this.#feedback("Session permanently deleted.");
    if (outcome.status === "stale") this.#feedback("Session changed. Review a fresh preview and confirm again.");
    this.#publish();
  }
  async #reconcile(): Promise<void> {
    ++this.#generation;
    this.#page = undefined;
    this.#publish(); // Invalidate every mounted selector's pre-mutation requests now.
    const { query, loaded } = this.#context;
    try {
      let page = await this.#client.listSessions(query, 0);
      const sessions = [...page.sessions];
      while (this.#live() && sessions.length < loaded && page.nextOffset !== undefined) {
        page = await this.#client.listSessions(query, page.nextOffset);
        sessions.push(...page.sessions);
      }
      if (this.#live()) this.#page = { sessions, nextOffset: page.nextOffset };
    } catch {
      if (this.#live()) {
        this.#page = { sessions: [] };
        this.#feedback("Session list unavailable. Reopen /resume to query native authority.");
      }
    }
  }
}
