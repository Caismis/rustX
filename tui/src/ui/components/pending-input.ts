import { PasteGuard } from "../paste-guard.ts";
import { Editor, matchesKey, Text, type TUI } from "@earendil-works/pi-tui";
import type { AppServerSession } from "../../app-server/session.ts";
import type { InboundDiagnostics, MethodParams } from "../../protocol/app-server.ts";
import { editorTheme } from "../theme.ts";
import type { PopupContent } from "./popup-frame.ts";

type Pending = NonNullable<InboundDiagnostics["pending"]>[number];
export function pendingText(item: Pending): string {
  return item.message.content.map(block => block.type === "text" ? block.text : "[attachment]").join("");
}
/** Observations are captured on opening; stale CAS is a visible terminal result. */
export class PendingInputView implements PopupContent {
  readonly editor: Editor;
  readonly items: Pending[];
  #focused = false;
  get focused(): boolean { return this.#focused; }
  set focused(value: boolean) { this.#focused = value; this.editor.focused = value; }
  #selected = 0;
  #editing = false;
  #busy = false;
  readonly #paste = new PasteGuard();
  #done = false;
  #notice = "";
  #height = 10;
  readonly session: AppServerSession;
  readonly close: () => void;
  readonly changed: () => void;
  constructor(tui: TUI, session: AppServerSession, close: () => void, changed: () => void) {
    this.session = session; this.close = close; this.changed = changed;
    this.items = structuredClone((session.state.inbound.pending ?? []).filter(item => item.message.source === "human"));
    this.editor = new Editor(tui, editorTheme);
    this.editor.onSubmit = () => {};
  }
  popupTitle(): string { return "Queued input"; }
  popupFooter(): string[] { return [this.#editing ? "Enter save · Esc back" : "↑↓ select · Enter edit text · Ctrl+D remove · Esc close"]; }
  setBodyHeight(height: number): void { this.#height = height; }
  invalidate(): void {}
  handleInput(data: string): void {
    if (this.#paste.content(data)) {
      if (this.#editing) this.editor.handleInput(data);
      return;
    }
    if (matchesKey(data, "escape")) { if (this.#editing) this.#editing = false; else this.close(); return; }
    if (this.#busy || this.#done) return;
    if (this.#editing) {
      // Pi trims its submit callback value and clears the editor first. Capture
      // the exact expanded draft before dispatch, including trailing newlines.
      const text = this.editor.getExpandedText();
      this.editor.onSubmit = () => { void this.mutate(text); };
      this.editor.handleInput(data);
      return;
    }
    if (matchesKey(data, "up")) this.#selected = Math.max(0, this.#selected - 1);
    else if (matchesKey(data, "down")) this.#selected = Math.min(this.items.length - 1, this.#selected + 1);
    else if (matchesKey(data, "ctrl+d")) void this.mutate();
    else if (matchesKey(data, "enter")) {
      const item = this.items[this.#selected];
      if (item && item.message.content.every(block => block.type === "text")) { this.#editing = true; this.editor.focused = this.#focused; this.editor.setText(pendingText(item)); }
      else this.#notice = "Only plain Human text can be edited; attachments can be removed.";
    }
    this.changed();
  }
  async mutate(text?: string): Promise<void> {
    const item = this.items[this.#selected];
    if (!item || this.#busy || this.#done) return;
    const expected: MethodParams<"inbound/remove">["expected"] = { sequence: item.sequence, message_id: item.message.id, revision: item.revision };
    this.#busy = true;
    try {
      if (text === undefined) await this.session.removePending(expected);
      else await this.session.editPending(expected, text);
      this.#notice = "Applied. Reopen to inspect the native queue.";
    } catch (error) { this.#notice = error instanceof Error ? error.message : String(error); }
    this.#editing = false;
    // No second action against a stale observation, including after unknown outcome.
    this.#done = true;
    this.#busy = false; this.changed();
  }
  render(width: number): string[] {
    const rows = this.#editing ? this.editor.render(width) : new Text(this.items.slice(this.#selected).map((item, offset) => `${offset === 0 ? "❯" : " "} ${pendingText(item)}`).join("\n") || "No queued Human input", 0, 0).render(width);
    return [...new Text(this.#notice, 0, 0).render(width), ...rows].slice(0, this.#height);
  }
}
