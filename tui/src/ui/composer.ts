/** Interaction only. Every execution/queue decision reads the native projection. */
import { PasteGuard } from "./paste-guard.ts";
import { Editor, matchesKey, type TUI } from "@earendil-works/pi-tui";
import { isAttemptActive, type PresentationState } from "../presentation/state.ts";
import { editorTheme } from "./theme.ts";
import type { ComposerProvenance } from "./composer-draft.ts";

export type ComposerIntent = "send" | "steer" | "queue";
export function composerIntent(state: PresentationState, queue = false): ComposerIntent {
  return isAttemptActive(state) ? queue ? "queue" : "steer" : "send";
}

/** Pi owns editing, paste expansion, history navigation and grapheme movement. */
export class ComposerEditor extends Editor {
  onQueue?: (text: string) => void;
  onPrompt?: (text: string, commands: boolean) => void;
  running: () => boolean = () => false;
  readonly #paste = new PasteGuard();
  #literalLeadingToken = false;
  #pasteAtTokenEnd: number | undefined;
  constructor(tui: TUI) {
    super(tui, editorTheme, { paddingX: 1 });
    this.onSubmit = () => {};
  }
  override handleInput(data: string): void {
    const content = this.#paste.content(data);
    if (content) {
      if (data.includes("\x1b[200~")) {
        this.#finishCommandPaste();
        // Pasting arguments after an explicitly authored command token keeps
        // command intent. Pasting into/before that token makes it literal.
        // Cursor offsets refer to Pi's own text representation, not cell width.
        const lines = this.getLines();
        const cursor = this.getCursor();
        const offset = lines.slice(0, cursor.line).reduce((n, line) => n + line.length + 1, 0) + cursor.col;
        const prefix = /^\s*\/\S+/.exec(lines.join("\n"));
        if (!prefix || offset < prefix[0].length) this.#literalLeadingToken = true;
        else if (offset === prefix[0].length) this.#pasteAtTokenEnd = offset;
      }
      super.handleInput(data);
      return;
    }
    this.#finishCommandPaste();
    if (!this.disableSubmit && this.running() && matchesKey(data, "tab")) {
      this.onQueue?.(this.getExpandedText());
      return;
    }
    const before = this.getExpandedText();
    const commands = !this.#literalLeadingToken;
    this.onSubmit = () => this.onPrompt?.(before, commands);
    super.handleInput(data);
    if (!this.getExpandedText()) this.#literalLeadingToken = false;
  }
  /** Pi assembles fragmented paste before the next keyboard action. At the
   * token boundary, whitespace starts arguments; any other insertion extends
   * the token. Empty paste does not change provenance.
   */
  #finishCommandPaste(): void {
    if (this.#pasteAtTokenEnd === undefined) return;
    const first = this.getExpandedText()[this.#pasteAtTokenEnd];
    if (first !== undefined && !/\s/.test(first)) this.#literalLeadingToken = true;
    this.#pasteAtTokenEnd = undefined;
  }
  captureProvenance(): ComposerProvenance {
    return { literal: this.#literalLeadingToken, pasteAtTokenEnd: this.#pasteAtTokenEnd };
  }
  /** Activating an existing draft restores its origin as well as its text.
   * Ordinary setText remains the explicit replacement boundary.
   */
  restoreDraftText(text: string, provenance: ComposerProvenance): void {
    this.#literalLeadingToken = Boolean(text) && provenance.literal;
    this.#pasteAtTokenEnd = text ? provenance.pasteAtTokenEnd : undefined;
    super.setText(text);
  }
  override setText(text: string): void {
    // Explicit replacement starts a new provenance boundary; restoring the
    // same text (for example after a busy submission) retains its provenance.
    if (!text || text !== this.getExpandedText()) {
      this.#literalLeadingToken = false;
      this.#pasteAtTokenEnd = undefined;
    }
    super.setText(text);
  }
}
