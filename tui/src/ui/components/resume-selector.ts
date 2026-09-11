/** Focused Session management presentation; deletion authority stays native. */
import { matchesKey, wrapTextWithAnsi } from "@earendil-works/pi-tui";
import type { RuntimeClientAttachment } from "../../runtime/attachment.ts";
import type { SessionDeletePreview, SessionDeleteResult, SessionSummaryView } from "../../protocol/types.ts";
import { sanitizeField } from "../../sanitize.ts";
import { ConfirmationView } from "./confirmation.ts";
import type { PopupContent } from "./popup-frame.ts";
import { SessionSelector, type SessionSelectorOptions } from "./session-selector.ts";

type Client = Pick<RuntimeClientAttachment, "listSessions" | "previewSessionDeletion" | "deleteSession" | "recoverSessionDeletion">;
type Operation =
  | { kind: "preview" | "recover"; sessionId: string }
  | { kind: "execute"; preview: Readonly<SessionDeletePreview> };
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
  readonly #client: Client;
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

  constructor(options: SessionSelectorOptions & { client: Client; alive: () => boolean; feedback: (text: string) => void }) {
    this.#client = options.client;
    this.#alive = options.alive;
    this.#feedback = options.feedback;
    this.#query = options.query ?? "";
    this.#nextOffset = options.nextOffset;
    this.selector = new SessionSelector(options);
    this.selector.onChange = () => this.onChange?.();
    this.selector.onCancel = () => this.onCancel?.();
    this.selector.onSelect = (session) => this.onSelect?.(session);
    this.selector.onQueryChange = (query) => { this.#query = query; void this.#rebuild(); };
    this.selector.onLoadMore = () => { void this.#loadMore(); };
    this.selector.onDelete = (id) => {
      if (this.#state.kind !== "selector") return;
      const rows = this.selector.visibleSessions();
      this.#anchor = { ids: rows.map((row) => row.id), index: rows.findIndex((row) => row.id === id), loaded: rows.length };
      void this.#request({ kind: "preview", sessionId: id });
    };
  }
  popupTitle(): string { return this.#state.kind === "selector" ? "Resume session" : "Session deletion"; }
  popupFooter(): string[] {
    if (this.#state.kind === "selector") return this.selector.popupFooter();
    if (this.#state.kind === "confirm") return this.#state.view.popupFooter();
    if (this.#state.kind === "pending") return this.#state.operation === "preview" ? ["Esc cancel"] : [];
    return [this.#state.kind === "notice" && this.#state.recoveryId ? "R retry native cleanup · Esc close" : "Esc close"];
  }
  invalidate(): void {}
  setBodyHeight(height: number): void { this.#bodyHeight = Math.max(1, height); }
  handleInput(data: string): void {
    if (!this.#alive()) return;
    const state = this.#state;
    if (state.kind === "selector") { this.selector.handleInput(data); return; }
    if (matchesKey(data, "escape")) {
      // Execute/recovery may already have committed. Keep focus and the request
      // serial until native settlement; local Esc cannot abandon that outcome.
      if (state.kind === "pending" && state.operation !== "preview") return;
      ++this.#workflowSerial;
      this.#state = { kind: "selector" };
    } else if (state.kind === "confirm") state.view.handleInput(data);
    else if (state.kind === "notice" && state.recoveryId && (data === "r" || data === "R")) {
      void this.#request({ kind: "recover", sessionId: state.recoveryId });
    }
    this.onChange?.();
  }
  render(width: number): string[] {
    const state = this.#state;
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
  async #request(request: Operation): Promise<void> {
    const operation = request.kind;
    const id = request.kind === "execute" ? request.preview.session_id : request.sessionId;
    const serial = ++this.#workflowSerial;
    this.#state = { kind: "pending", operation };
    this.onChange?.();
    try {
      const result = request.kind === "execute" ? await this.#client.deleteSession(id, request.preview.target_revision)
        : request.kind === "preview" ? await this.#client.previewSessionDeletion(id)
        : await this.#client.recoverSessionDeletion(id);
      if (!this.#alive() || serial !== this.#workflowSerial) return;
      await this.#result(result, id);
    } catch {
      if (!this.#alive() || serial !== this.#workflowSerial) return;
      this.#state = { kind: "notice", text: operation === "preview"
        ? "Preview unavailable. Reconciling the Session list; no deletion was submitted."
        : "Deletion outcome unknown. Rechecking native Session visibility; this is not proof of failure.",
        ...(operation === "preview" ? {} : { recoveryId: id }) };
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
          onConfirm: () => { void this.#request({ kind: "execute", preview }); },
        });
        this.#state = { kind: "confirm", preview, view };
        break;
      }
      case "stale":
        this.#feedback("Session changed. Review a fresh preview and confirm again.");
        await this.#request({ kind: "preview", sessionId: id });
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
        this.#feedback("Session permanently deleted.");
        await this.#rebuild(this.#anchor);
        break;
      case "committed_cleanup_pending":
        this.#state = { kind: "notice", recoveryId: id, text: "The Session has been removed and cannot be resumed, but some local data still needs cleanup. Press R to retry native cleanup." };
        await this.#rebuild(this.#anchor);
        break;
      case "committed_durability_uncertain":
        this.#state = { kind: "notice", recoveryId: id, text: "Deletion visibility may already be committed, but durability is uncertain. Rechecking native state. Press R for native recovery." };
        await this.#rebuild(this.#anchor);
        break;
      case "not_found":
        this.#state = { kind: "notice", text: "Session is absent from native authority; it may have been removed elsewhere. Refreshing the list." };
        await this.#rebuild(this.#anchor);
        break;
    }
  }
  async #rebuild(anchor?: Anchor): Promise<void> {
    const serial = ++this.#requestSerial;
    this.#nextOffset = undefined;
    this.#listBusy = true;
    // Clear an untrusted generation, never splice a target row.
    this.selector.replacePage([]);
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
      this.selector.replacePage(rows, page.nextOffset);
      if (anchor) {
        const ids = new Set(rows.map((row) => row.id));
        const target = anchor.ids[anchor.index];
        const next = anchor.ids.slice(anchor.index + 1).find((id) => ids.has(id));
        const previous = anchor.ids.slice(0, anchor.index).reverse().find((id) => ids.has(id));
        this.selector.selectIdentity(target && ids.has(target) ? target : next ?? rows[anchor.index]?.id ?? previous ?? rows.at(-1)?.id);
      }
    } catch {
      if (this.#alive() && serial === this.#requestSerial) this.#feedback("Session list unavailable. Reopen /resume to query native authority.");
    } finally {
      if (serial === this.#requestSerial) this.#listBusy = false;
      if (this.#alive()) this.onChange?.();
    }
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
