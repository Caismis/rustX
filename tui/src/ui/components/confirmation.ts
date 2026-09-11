/** A bounded confirmation surface for one destructive runtime operation. */

import {
  matchesKey,
  truncateToWidth,
  wrapTextWithAnsi,
  type Focusable,
} from "@earendil-works/pi-tui";

import { sanitizeField } from "../../sanitize.ts";
import { role } from "../theme.ts";
import type { PopupContent } from "./popup-frame.ts";

export interface ConfirmationViewOptions {
  title: string;
  subject: string;
  warning: string;
  confirmLabel: string;
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
  #confirmSelected = false;
  readonly #confirmLabel: string;

  constructor(options: ConfirmationViewOptions) {
    this.#confirmLabel = options.confirmLabel;
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
    return ["←→/Tab choose · Enter activate · Esc cancel"];
  }

  setBodyHeight(height: number): void {
    this.#bodyHeight = Math.max(1, Math.floor(height));
  }

  invalidate(): void {}

  handleInput(data: string): void {
    if (this.#acted) return;
    if (matchesKey(data, "left") || matchesKey(data, "right") || matchesKey(data, "tab")) {
      this.#confirmSelected = !this.#confirmSelected;
      return;
    }
    if (matchesKey(data, "enter")) {
      this.#acted = true;
      if (this.#confirmSelected) this.#onConfirm();
      else this.#onCancel();
      return;
    }
    if (matchesKey(data, "escape") || data === "n" || data === "N") {
      this.#acted = true;
      this.#onCancel();
    }
  }

  render(width: number): string[] {
    const choices = [
      `${this.#confirmSelected ? " " : "❯"} Cancel`,
      `${this.#confirmSelected ? "❯" : " "} ${this.#confirmLabel}`,
    ];
    const lines = [
      ...(this.#bodyHeight === 1 ? [choices[this.#confirmSelected ? 1 : 0]!] : choices),
      role.strong(sanitizeField(this.#subject)),
      "Permanent: this cannot be undone through rustX.",
      ...wrapTextWithAnsi(role.warning(sanitizeField(this.#warning)), Math.max(1, width)),
    ];
    return lines
      .slice(0, this.#bodyHeight)
      .map((line) => truncateToWidth(line, Math.max(1, width), "…"));
  }
}
