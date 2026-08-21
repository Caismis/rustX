/**
 * A focused, bounded view for substantial read-only client information.
 *
 * The dispatcher supplies the intent and the Markdown body. This component
 * owns only Pi rendering mechanics and a local viewport cursor; it never
 * writes to the runtime projection or canonical conversation history.
 */

import {
  Key,
  Markdown,
  matchesKey,
  truncateToWidth,
  type Component,
  type Focusable,
} from "@earendil-works/pi-tui";

import { markdownTheme, role } from "../theme.ts";

export interface InspectionViewOptions {
  title: string;
  body: string;
  /** Number of Markdown-rendered body lines visible at once. */
  viewportLines: number;
}

/** One reusable scrollable inspection surface for all read-only commands. */
export class InspectionView implements Component, Focusable {
  focused = false;
  onClose?: () => void;
  onChange?: () => void;

  readonly #title: string;
  readonly #viewportLines: number;
  readonly #markdown: Markdown;
  #offset = 0;
  #bodyLineCount = 0;

  constructor(options: InspectionViewOptions) {
    this.#title = options.title;
    this.#viewportLines = Math.max(1, Math.floor(options.viewportLines));
    this.#markdown = new Markdown(options.body, 0, 0, markdownTheme);
  }

  /** Current first visible body line, exposed for deterministic component tests. */
  get offset(): number {
    return this.#offset;
  }

  /** Number of rendered body lines at the last width passed to `render`. */
  get bodyLineCount(): number {
    return this.#bodyLineCount;
  }

  handleInput(data: string): void {
    if (matchesKey(data, Key.escape)) {
      this.onClose?.();
      return;
    }
    if (matchesKey(data, Key.up)) {
      this.#setOffset(this.#offset - 1);
      return;
    }
    if (matchesKey(data, Key.down)) {
      this.#setOffset(this.#offset + 1);
      return;
    }
    if (matchesKey(data, Key.pageUp)) {
      this.#setOffset(this.#offset - this.#viewportLines);
      return;
    }
    if (matchesKey(data, Key.pageDown)) {
      this.#setOffset(this.#offset + this.#viewportLines);
      return;
    }
    if (matchesKey(data, Key.home)) {
      this.#setOffset(0);
      return;
    }
    if (matchesKey(data, Key.end)) {
      this.#setOffset(this.#maxOffset());
    }
  }

  render(width: number): string[] {
    const safeWidth = Math.max(1, width);
    const body = this.#markdown.render(safeWidth);
    const bodyLines = body.length === 0 ? ["(empty inspection)"] : body;
    this.#bodyLineCount = bodyLines.length;
    this.#offset = Math.min(this.#offset, this.#maxOffset());

    const first = this.#offset + 1;
    const last = Math.min(
      this.#offset + this.#viewportLines,
      this.#bodyLineCount,
    );
    const lines = [
      role.strong(this.#title),
      role.meta(`lines ${first}-${last} of ${this.#bodyLineCount}`),
      ...bodyLines.slice(this.#offset, this.#offset + this.#viewportLines),
      role.meta("↑↓ scroll · PageUp/PageDown · Home/End · Esc close"),
    ];
    return lines.map((line) => truncateToWidth(line, safeWidth, "…"));
  }

  invalidate(): void {
    this.#markdown.invalidate();
  }

  #maxOffset(): number {
    return Math.max(0, this.#bodyLineCount - this.#viewportLines);
  }

  #setOffset(offset: number): void {
    const next = Math.max(0, Math.min(offset, this.#maxOffset()));
    if (next === this.#offset) {
      return;
    }
    this.#offset = next;
    this.onChange?.();
  }
}
