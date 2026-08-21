/**
 * A single, finite client-feedback surface.
 *
 * This is deliberately not a history container: replacement keeps one item,
 * acknowledgement clears it, and the app owns its lifetime outside the
 * runtime-derived presentation projection.
 */

import {
  truncateToWidth,
  type Component,
} from "@earendil-works/pi-tui";

import { role } from "../theme.ts";

export type TransientFeedbackLevel = "info" | "error";

export interface TransientFeedback {
  level: TransientFeedbackLevel;
  text: string;
}

const MAX_LINES = 3;

export class TransientFeedbackSurface implements Component {
  #feedback: TransientFeedback | undefined;

  get feedback(): TransientFeedback | undefined {
    return this.#feedback;
  }

  /** New feedback replaces the previous item; it never accumulates. */
  replace(feedback: TransientFeedback): void {
    this.#feedback = feedback;
  }

  /** A user action acknowledges the current item. */
  acknowledge(): void {
    this.#feedback = undefined;
  }

  clear(): void {
    this.#feedback = undefined;
  }

  render(width: number): string[] {
    const feedback = this.#feedback;
    if (feedback === undefined) {
      return [];
    }

    const sourceLines = feedback.text.split(/\r?\n/);
    const lines = sourceLines.slice(0, MAX_LINES);
    if (sourceLines.length > MAX_LINES) {
      lines[MAX_LINES - 1] = `${lines[MAX_LINES - 1] ?? ""} …`;
    }
    const color = feedback.level === "error" ? role.error : role.assistant;
    return lines.map((line) => truncateToWidth(color(line), Math.max(1, width), "…"));
  }

  invalidate(): void {
    // Rendering is derived directly from the one current feedback item.
  }
}
