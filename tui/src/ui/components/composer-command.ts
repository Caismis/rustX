import { Input } from "@earendil-works/pi-tui";
import type { PopupContent } from "./popup-frame.ts";

/** A command entry surface that leaves the Composer, cursor and receipts intact. */
export class ComposerCommand implements PopupContent {
  readonly input = new Input();
  constructor(submit: (text: string) => void, close: () => void) {
    this.input.onSubmit = submit;
    this.input.onEscape = close;
    this.input.handleInput("/");
  }
  get focused(): boolean { return this.input.focused; }
  set focused(value: boolean) { this.input.focused = value; }
  popupTitle(): string { return "Composer command"; }
  popupFooter(): string[] { return ["/attach <path> · /queue", "Enter open · Esc return to draft"]; }
  setBodyHeight(_height: number): void {}
  invalidate(): void {}
  handleInput(data: string): void { this.input.handleInput(data); }
  render(width: number): string[] { return this.input.render(width); }
}
