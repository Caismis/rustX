/** Interaction only. Every execution/queue decision reads the native projection. */
import { PasteGuard } from "./paste-guard.ts";
import { Editor, matchesKey, type TUI } from "@earendil-works/pi-tui";
import { isAttemptActive, type PresentationState } from "../presentation/state.ts";
import { editorTheme } from "./theme.ts";

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
  #literal = false;
  constructor(tui: TUI) {
    super(tui, editorTheme, { paddingX: 1 });
    this.onSubmit = () => {};
  }
  override handleInput(data: string): void {
    const content = this.#paste.content(data);
    if (content) {
      this.#literal = true;
      super.handleInput(data);
      return;
    }
    if (!this.disableSubmit && this.running() && matchesKey(data, "tab")) {
      this.onQueue?.(this.getExpandedText());
      return;
    }
    const before = this.getExpandedText();
    const commands = !this.#literal;
    this.onSubmit = () => this.onPrompt?.(before, commands);
    super.handleInput(data);
    if (!this.getExpandedText()) this.#literal = false;
  }
  override setText(text: string): void {
    super.setText(text);
    if (!text) this.#literal = false;
  }
}
