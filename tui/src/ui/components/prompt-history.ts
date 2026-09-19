import { PasteGuard } from "../paste-guard.ts";
import { Input, matchesKey, Text } from "@earendil-works/pi-tui";
import type { PopupContent } from "./popup-frame.ts";

/** Local submitted prompts only; commands never enter this list. */
export class PromptHistory implements PopupContent {
  readonly input = new Input();
  #selected = 0;
  #height = 8;
  readonly #paste = new PasteGuard();
  readonly prompts: readonly string[];
  readonly accept: (text: string) => void;
  readonly close: () => void;
  constructor(prompts: readonly string[], accept: (text: string) => void, close: () => void) {
    this.prompts = prompts; this.accept = accept; this.close = close;
  }
  get focused(): boolean { return this.input.focused; }
  set focused(value: boolean) { this.input.focused = value; }
  popupTitle(): string { return "Prompt history"; }
  popupFooter(): string[] { return ["↑↓ preview · Enter edit · Esc restore draft"]; }
  setBodyHeight(height: number): void { this.#height = height; }
  invalidate(): void {}
  get matches(): string[] { return [...this.prompts].reverse().filter(text => text.toLocaleLowerCase().includes(this.input.getValue().toLocaleLowerCase())); }
  handleInput(data: string): void {
    if (this.#paste.content(data)) {
      this.input.handleInput(data); this.#selected = 0; return;
    }
    if (matchesKey(data, "escape")) { this.close(); return; }
    if (matchesKey(data, "enter")) { const text = this.matches[this.#selected]; if (text !== undefined) this.accept(text); return; }
    if (matchesKey(data, "up")) this.#selected = Math.max(0, this.#selected - 1);
    else if (matchesKey(data, "down")) this.#selected = Math.min(this.matches.length - 1, this.#selected + 1);
    else { this.input.handleInput(data); this.#selected = 0; }
  }
  render(width: number): string[] {
    return [...this.input.render(width), ...new Text(this.matches[this.#selected] ?? "No matching prompts", 0, 0).render(width)].slice(0, this.#height);
  }
}
