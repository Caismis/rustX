/** A bounded confirmation surface for one destructive runtime operation. */

import {
  matchesKey,
  truncateToWidth,
  type Focusable,
} from "@earendil-works/pi-tui";

import { role } from "../theme.ts";
import type { PopupContent } from "./popup-frame.ts";

export interface ConfirmationViewOptions {
  title: string;
  subject: string;
  warning: string;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * One-shot confirmation for a destructive action owned by the runtime.
 *
 * The view contains no resource identity beyond the human-readable subject
 * supplied by its caller. It never receives a path, branch, or other physical
 * Git fact, and it never performs the operation itself.
 */
export class ConfirmationView implements PopupContent, Focusable {
  focused = false;

  readonly #title: string;
  readonly #subject: string;
  readonly #warning: string;
  readonly #onConfirm: () => void;
  readonly #onCancel: () => void;
  #bodyHeight = 8;
  #acted = false;

  constructor(options: ConfirmationViewOptions) {
    this.#title = options.title;
    this.#subject = options.subject;
    this.#warning = options.warning;
    this.#onConfirm = options.onConfirm;
    this.#onCancel = options.onCancel;
  }

  popupTitle(): string {
    return this.#title;
  }

  popupFooter(): string[] {
    return ["Enter/Y confirm · Esc/N cancel"];
  }

  setBodyHeight(height: number): void {
    this.#bodyHeight = Math.max(1, Math.floor(height));
  }

  invalidate(): void {}

  handleInput(data: string): void {
    if (this.#acted) return;
    if (matchesKey(data, "enter") || data === "y" || data === "Y") {
      this.#acted = true;
      this.#onConfirm();
      return;
    }
    if (matchesKey(data, "escape") || data === "n" || data === "N") {
      this.#acted = true;
      this.#onCancel();
    }
  }

  render(width: number): string[] {
    const lines = [
      role.strong(this.#subject),
      "",
      role.warning(this.#warning),
      "",
      "This cannot be undone by the runtime.",
    ];
    return lines
      .slice(0, this.#bodyHeight)
      .map((line) => truncateToWidth(line, Math.max(1, width), "…"));
  }
}
