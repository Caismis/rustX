/** Focused Session management presentation; deletion authority stays native. */
import { matchesKey, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import { SessionDeletionWorkflow, type DeletionClient, type DeletionContext, type ReconciledSessions } from "../session-deletion-workflow.ts";
import type { SessionDeletePreview, SessionDeleteResult, SessionSummaryView } from "../../protocol/types.ts";
import { sanitizeField } from "../../sanitize.ts";
import { ConfirmationView } from "./confirmation.ts";
import type { PopupContent } from "./popup-frame.ts";
import { SessionSelector, type SessionSelectorOptions } from "./session-selector.ts";

type State =
  | { kind: "selector" }
  | { kind: "pending"; operation: "preview" | "execute" | "recover" }
  | { kind: "confirm"; preview: Readonly<SessionDeletePreview>; view: ConfirmationView }
  | { kind: "notice"; text: string; recoveryId?: string };
interface Anchor { ids: string[]; index: number; loaded: number }

export class ResumeSelector implements PopupContent {
  focused = false;
  readonly selector: SessionSelector;
  onChange?: () => void;
  onCancel?: () => void;
  onSelect?: (session: SessionSummaryView) => void;
  readonly #client: DeletionClient;
  readonly #workflow: SessionDeletionWorkflow;
  readonly #unsubscribe: () => void;
  #generation: number;
  #listStatus: "ready" | "pending" | "unavailable";
  #appliedPage: ReconciledSessions | undefined;
  readonly #alive: () => boolean;
  readonly #feedback: (text: string) => void;
  #state: State = { kind: "selector" };
  #query: string;
  #nextOffset: number | undefined;
  // Search/reconciliation invalidates older query AND continuation responses.
  #requestSerial = 0;
  #workflowSerial = 0;
  #listBusy = false;
  #bodyHeight = 24;
  #anchor: Anchor = { ids: [], index: 0, loaded: 0 };

  constructor(options: Omit<SessionSelectorOptions, "sessions" | "nextOffset"> & { initialPage?: ReconciledSessions; client: DeletionClient; workflow: SessionDeletionWorkflow; alive: () => boolean; feedback: (text: string) => void }) {
    this.#client = options.client;
    this.#workflow = options.workflow;
    // A fresh initial response already belongs to this generation. Mounting is
    // not a mutation, and an older workflow page must not overwrite that read.
    this.#generation = this.#workflow.generation;
    this.#listStatus = options.initialPage ? "ready" : "unavailable";
    if (options.initialPage && this.#workflow.reconciliation.kind === "ready") {
      this.#appliedPage = this.#workflow.reconciliation.page;
    }
    if (this.#workflow.state.kind !== "idle" && this.#workflow.context.query === (options.query ?? "")) {
      const { ids, index, loaded } = this.#workflow.context;
      this.#anchor = { ids: [...ids], index, loaded };
    }
    this.#alive = options.alive;
    this.#feedback = options.feedback;
    this.#query = options.query ?? "";
    this.#nextOffset = options.initialPage?.nextOffset;
    this.selector = new SessionSelector({ ...options, sessions: options.initialPage?.sessions ?? [], nextOffset: this.#nextOffset });
    if (options.initialPage && this.#workflow.state.kind !== "idle" && this.#workflow.context.query === this.#query) this.#restoreSelection(this.#anchor);
    this.selector.onChange = () => this.onChange?.();
    this.selector.onCancel = () => this.onCancel?.();
    this.selector.onSelect = (session) => this.onSelect?.(session);
    this.selector.onQueryChange = (query) => { this.#query = query; void this.#rebuild(); };
    this.selector.onLoadMore = () => { void this.#loadMore(); };
    this.selector.onDelete = (id) => {
      if (this.#state.kind !== "selector") return;
      if (this.#workflow.canRecover()) { this.#syncWorkflow(); return; }
      const rows = this.selector.visibleSessions();
      this.#anchor = { ids: rows.map((row) => row.id), index: rows.findIndex((row) => row.id === id), loaded: rows.length };
      void this.#preview(id);
    };
    this.#unsubscribe = this.#workflow.subscribe(() => this.#syncWorkflow());
    this.#syncWorkflow();
  }
  /** Current view context is not recovery authority. Unknown visibility has no anchor. */
  reconciliationContext(): DeletionContext {
    const rows = this.#listStatus === "ready" ? this.selector.visibleSessions() : [];
    const selected = this.#listStatus === "ready" ? this.selector.selectedSession()?.id : undefined;
    return { query: this.#query, ids: rows.map((row) => row.id),
      index: Math.max(0, rows.findIndex((row) => row.id === selected)), loaded: rows.length };
  }
  dispose(): void { ++this.#workflowSerial; ++this.#requestSerial; this.#unsubscribe(); }
  popupTitle(): string { return this.#state.kind === "selector" ? "Resume session" : "Session deletion"; }
  popupFooter(): string[] {
    if (this.#state.kind === "selector") return this.#listStatus === "ready" ? this.selector.popupFooter() : ["Esc close"];
    if (this.#state.kind === "confirm") return this.#state.view.popupFooter();
    if (this.#state.kind === "pending") return this.#state.operation === "preview" ? ["Esc cancel"] : [];
    return [this.#state.kind === "notice" && this.#state.recoveryId ? "R retry native cleanup · Esc close" : "Esc close"];
  }
  invalidate(): void {}
  setBodyHeight(height: number): void { this.#bodyHeight = Math.max(1, height); }
  handleInput(data: string): void {
    if (!this.#alive()) return;
    const state = this.#state;
    if (state.kind === "selector") {
      // Search remains editable while a native query is pending/unavailable,
      // but old rows cannot be navigated, resumed, or selected for deletion.
      if (this.#listStatus !== "ready" && (["ctrl+d", "enter", "up", "down"] as const).some((key) => matchesKey(data, key))) return;
      this.selector.handleInput(data);
      return;
    }
    if (matchesKey(data, "escape")) {
      // Execute/recovery may already have committed. Keep focus and the request
      // serial until native settlement; local Esc cannot abandon that outcome.
      if (state.kind === "pending" && state.operation !== "preview") return;
      this.#workflow.dismiss();
      ++this.#workflowSerial;
      this.#state = { kind: "selector" };
    } else if (state.kind === "confirm") state.view.handleInput(data);
    else if (state.kind === "notice" && state.recoveryId && (data === "r" || data === "R")) {
      this.#workflow.recover(this.reconciliationContext());
    }
    this.onChange?.();
  }
  render(width: number): string[] {
    const state = this.#state;
    if (state.kind === "selector" && this.#listStatus !== "ready") {
      const text = this.#listStatus === "pending" ? "Refreshing native Session visibility…"
        : "Session visibility unavailable. Reopen /resume to query native authority.";
      return wrapTextWithAnsi(text, Math.max(1, width)).slice(0, this.#bodyHeight);
    }
    if (state.kind === "selector") {
      this.selector.focused = this.focused;
      this.selector.setBodyHeight(this.#bodyHeight);
      return this.selector.render(width);
    }
    if (state.kind === "confirm") {
      state.view.setBodyHeight(this.#bodyHeight);
      return state.view.render(width);
    }
    const text = state.kind === "pending"
      ? `Waiting for native ${state.operation === "execute" ? "deletion" : state.operation === "recover" ? "cleanup" : "preview"}…`
      : state.text;
    return wrapTextWithAnsi(sanitizeField(text), Math.max(1, width)).slice(0, this.#bodyHeight);
  }
  #syncWorkflow(): void {
    const workflow = this.#workflow;
    if (workflow.state.kind === "idle") return;
    if (workflow.generation !== this.#generation) {
      this.#generation = workflow.generation;
      ++this.#requestSerial;
      this.#nextOffset = undefined;
      this.#listBusy = false;
      this.#listStatus = "unavailable";
    }
    const reconciliation = workflow.reconciliation;
    if (reconciliation.kind === "pending" && this.#listStatus !== "ready") this.#listStatus = "pending";
    else if (reconciliation.kind === "failed" && this.#listStatus === "pending") this.#listStatus = "unavailable";
    const page = reconciliation.kind === "ready" && reconciliation.query === this.#query ? reconciliation.page : undefined;
    if (page && page !== this.#appliedPage) {
      this.#appliedPage = page;
      this.#listStatus = "ready";
      this.#nextOffset = page.nextOffset;
      this.selector.replacePage(page.sessions, page.nextOffset);
      this.#restoreSelection(workflow.context);
    }
    const state = workflow.state;
    if (state.kind === "pending") this.#state = { kind: "pending", operation: state.operation };
    else if (state.kind === "result") {
      if (state.outcome.status === "stale") {
        // A new preview belongs to this surface, and disappears with it.
        const id = workflow.takeStale();
        if (id) void this.#preview(id);
      } else if (state.outcome.status === "unknown") {
        this.#state = { kind: "notice", recoveryId: state.sessionId, text: "Deletion outcome unknown. Rechecking native Session visibility; this is not proof of failure. Press R for native recovery." };
      } else void this.#result(state.outcome, state.sessionId);
    }
    this.onChange?.();
  }
  async #preview(id: string): Promise<void> {
    const serial = ++this.#workflowSerial;
    this.#state = { kind: "pending", operation: "preview" };
    this.onChange?.();
    try {
      const result = await this.#client.previewSessionDeletion(id);
      if (!this.#alive() || serial !== this.#workflowSerial) return;
      await this.#result(result, id);
      if (result.status === "not_found") await this.#rebuild(this.#anchor);
    } catch {
      if (!this.#alive() || serial !== this.#workflowSerial) return;
      this.#state = { kind: "notice", text: "Preview unavailable. Reconciling the Session list; no deletion was submitted." };
      await this.#rebuild(this.#anchor);
    }
    if (this.#alive()) this.onChange?.();
  }
  async #result(result: SessionDeleteResult, id: string): Promise<void> {
    switch (result.status) {
      case "preview": {
        const preview = Object.freeze({ ...result.preview });
        const view = new ConfirmationView({
          title: "Permanently delete Session?", confirmLabel: "Permanently delete",
          subject: `${preview.name ?? "Unnamed Session"} · ${preview.session_id}`,
          warning: `Deletes ${preview.owned_node_count} nodes, ${preview.owned_conversation_count} conversations, ${preview.owned_child_count} children. Independent fork/clone Sessions and project files are preserved.`,
          onCancel: () => { this.#state = { kind: "selector" }; },
          onConfirm: () => { this.#workflow.execute(preview, { ...this.#anchor, query: this.#query }); },
        });
        this.#state = { kind: "confirm", preview, view };
        break;
      }
      case "stale":
        this.#feedback("Session changed. Review a fresh preview and confirm again.");
        await this.#preview(id);
        break;
      case "blocked": {
        const reason = result.reason;
        const text = reason.kind === "current_session" ? "The active Session cannot be deleted in this version. Switch Sessions or use /new first."
          : reason.kind === "in_use" ? "The Session or an owned child is currently in use. Release it before trying again."
          : reason.kind === "workspace" ? `${reason.resource_count} retained workspace resources block deletion. Use the existing workspace/subagent disposal action explicitly first.`
          : "Native ownership could not be validated. The Session remains available; deletion is blocked.";
        this.#state = { kind: "notice", text };
        break;
      }
      case "deleted":
        this.#state = { kind: "selector" };
        this.#workflow.dismiss();
        break;
      case "committed_cleanup_pending":
        this.#state = { kind: "notice", recoveryId: id, text: "The Session has been removed and cannot be resumed, but some local data still needs cleanup. Press R to retry native cleanup." };
        break;
      case "committed_durability_uncertain":
        this.#state = { kind: "notice", recoveryId: id, text: "Deletion visibility may already be committed, but durability is uncertain. Rechecking native state. Press R for native recovery." };
        break;
      case "not_found":
        this.#state = { kind: "notice", text: "Session is absent from native authority; it may have been removed elsewhere. Refreshing the list." };
        break;
    }
  }
  async #rebuild(anchor?: Anchor): Promise<void> {
    const serial = ++this.#requestSerial;
    this.#nextOffset = undefined;
    this.#listBusy = true;
    // Hide the untrusted generation; absence of rows is only native truth on success.
    this.#listStatus = "pending";
    const query = this.#query;
    try {
      let page = await this.#client.listSessions(query, 0);
      const rows = [...page.sessions];
      while (this.#alive() && serial === this.#requestSerial && anchor && rows.length < anchor.loaded && page.nextOffset !== undefined) {
        page = await this.#client.listSessions(query, page.nextOffset);
        rows.push(...page.sessions);
      }
      if (!this.#alive() || serial !== this.#requestSerial) return;
      this.#nextOffset = page.nextOffset;
      this.#listStatus = "ready";
      this.selector.replacePage(rows, page.nextOffset);
      if (anchor) this.#restoreSelection(anchor);
    } catch {
      if (this.#alive() && serial === this.#requestSerial) {
        this.#listStatus = "unavailable";
        this.#feedback("Session visibility could not be refreshed. Reopen /resume to query native authority.");
      }
    } finally {
      if (serial === this.#requestSerial) this.#listBusy = false;
      if (this.#alive()) this.onChange?.();
    }
  }
  #restoreSelection(anchor: Anchor): void {
    const rows = this.selector.visibleSessions();
    const ids = new Set(rows.map((row) => row.id));
    const target = anchor.ids[anchor.index];
    const next = anchor.ids.slice(anchor.index + 1).find((id) => ids.has(id));
    const previous = anchor.ids.slice(0, anchor.index).reverse().find((id) => ids.has(id));
    this.selector.selectIdentity(target && ids.has(target) ? target : next ?? rows[anchor.index]?.id ?? previous ?? rows.at(-1)?.id);
  }
  async #loadMore(): Promise<void> {
    if (this.#listBusy || this.#nextOffset === undefined) return;
    const serial = this.#requestSerial;
    this.#listBusy = true;
    try {
      const page = await this.#client.listSessions(this.#query, this.#nextOffset);
      if (!this.#alive() || serial !== this.#requestSerial) return;
      this.#nextOffset = page.nextOffset;
      this.selector.appendPage(page.sessions, page.nextOffset);
    } catch {
      if (this.#alive() && serial === this.#requestSerial) {
        this.selector.appendPage([], this.#nextOffset);
        this.#feedback("Session page unavailable. Try again.");
      }
    } finally { if (serial === this.#requestSerial) this.#listBusy = false; }
  }
}
