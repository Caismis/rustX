/**
 * Pi-like searchable selectors for native Rust-owned Sessions and historical
 * user-message boundaries. These components own only query/focus/selection;
 * all authoritative metadata and branch choices arrive from Runtime Client.
 */

import {
  fuzzyMatch,
  matchesKey,
  truncateToWidth,
  type Component,
  type Focusable,
} from "@earendil-works/pi-tui";

import type {
  SessionSummaryView,
  SessionUserMessageBoundaryView,
} from "../../protocol/types.ts";
import { role, style } from "../theme.ts";

const VISIBLE_ROWS = 8;

export interface SessionSelectorOptions {
  sessions: SessionSummaryView[];
}

export class SessionSelector implements Component, Focusable {
  focused = false;
  onSelect?: (session: SessionSummaryView) => void;
  onCancel?: () => void;
  onChange?: () => void;

  readonly #sessions: SessionSummaryView[];
  #query = "";
  #selected = 0;

  constructor(options: SessionSelectorOptions) {
    this.#sessions = options.sessions;
  }

  invalidate(): void {
    // The selector has no cached render state.
  }

  visibleSessions(): SessionSummaryView[] {
    if (this.#query.length === 0) {
      return this.#sessions;
    }
    return this.#sessions
      .map((session, index) => ({ session, index }))
      .filter(({ session }) =>
        fuzzyMatch(this.#query, sessionSearchText(session)).matches,
      )
      .sort((left, right) => left.index - right.index)
      .map(({ session }) => session);
  }

  handleInput(data: string): void {
    const visible = this.visibleSessions();
    if (matchesKey(data, "escape")) {
      this.onCancel?.();
    } else if (matchesKey(data, "enter")) {
      const selected = visible[this.#selected];
      if (selected !== undefined) this.onSelect?.(selected);
    } else if (matchesKey(data, "up")) {
      this.#move(-1, visible.length);
    } else if (matchesKey(data, "down")) {
      this.#move(1, visible.length);
    } else if (matchesKey(data, "backspace")) {
      this.#query = this.#query.slice(0, -1);
      this.#selected = 0;
      this.onChange?.();
    } else if (isPrintable(data)) {
      this.#query += data;
      this.#selected = 0;
      this.onChange?.();
    }
  }

  render(width: number): string[] {
    const visible = this.visibleSessions();
    const lines = [
      role.strong("Resume session"),
      `${role.meta("Search:")} ${this.#query}${this.focused ? role.accent("▌") : ""}`,
      "",
    ];
    if (visible.length === 0) {
      lines.push(role.meta("no persisted session matches the search"));
    } else {
      const start = Math.max(
        0,
        Math.min(
          this.#selected - Math.floor(VISIBLE_ROWS / 2),
          Math.max(0, visible.length - VISIBLE_ROWS),
        ),
      );
      for (const [offset, session] of visible
        .slice(start, start + VISIBLE_ROWS)
        .entries()) {
        const index = start + offset;
        const marker = index === this.#selected ? role.accent("❯") : " ";
        const active = session.active ? role.success(" active") : "";
        lines.push(`${marker} ${index === this.#selected ? style.bold(session.name) : session.name}${active}`);
        lines.push(`    ${role.meta(`${session.id} · node ${session.active_node}`)}`);
      }
      if (visible.length > VISIBLE_ROWS) {
        lines.push(role.meta(`${this.#selected + 1}/${visible.length}`));
      }
    }
    lines.push("", role.meta("↑↓ navigate · Enter select · Esc close"));
    return lines.map((line) => truncateToWidth(line, width, "…"));
  }

  #move(delta: number, length: number): void {
    if (length === 0) return;
    this.#selected = (this.#selected + delta + length) % length;
    this.onChange?.();
  }
}

export interface BoundarySelectorOptions {
  boundaries: SessionUserMessageBoundaryView[];
  title: string;
}

export class BoundarySelector implements Component, Focusable {
  focused = false;
  onSelect?: (boundary: SessionUserMessageBoundaryView) => void;
  onCancel?: () => void;
  onChange?: () => void;

  readonly #boundaries: SessionUserMessageBoundaryView[];
  readonly #title: string;
  #query = "";
  #selected = 0;

  constructor(options: BoundarySelectorOptions) {
    this.#boundaries = options.boundaries;
    this.#title = options.title;
    this.#selected = Math.max(0, this.#boundaries.length - 1);
  }

  invalidate(): void {
    // The selector has no cached render state.
  }

  private visible(): SessionUserMessageBoundaryView[] {
    if (this.#query.length === 0) return this.#boundaries;
    return this.#boundaries.filter((boundary) =>
      fuzzyMatch(this.#query, boundarySearchText(boundary)).matches,
    );
  }

  handleInput(data: string): void {
    const visible = this.visible();
    if (matchesKey(data, "escape")) this.onCancel?.();
    else if (matchesKey(data, "enter")) {
      const boundary = visible[this.#selected];
      if (boundary !== undefined) this.onSelect?.(boundary);
    } else if (matchesKey(data, "up")) this.#move(-1, visible.length);
    else if (matchesKey(data, "down")) this.#move(1, visible.length);
    else if (matchesKey(data, "backspace")) {
      this.#query = this.#query.slice(0, -1);
      this.#selected = 0;
      this.onChange?.();
    } else if (isPrintable(data)) {
      this.#query += data;
      this.#selected = 0;
      this.onChange?.();
    }
  }

  render(width: number): string[] {
    const visible = this.visible();
    const lines = [
      role.strong(this.#title),
      role.meta("Select a committed user boundary; the prompt will return to the editor."),
      `${role.meta("Search:")} ${this.#query}${this.focused ? role.accent("▌") : ""}`,
      "",
    ];
    if (visible.length === 0) {
      lines.push(role.meta("no branchable user message matches the search"));
    } else {
      const start = Math.max(0, Math.min(this.#selected - 3, visible.length - VISIBLE_ROWS));
      visible.slice(start, start + VISIBLE_ROWS).forEach((boundary, offset) => {
        const index = start + offset;
        const message = previewUserMessage(boundary);
        const marker = index === this.#selected ? role.accent("❯") : " ";
        lines.push(`${marker} ${index === this.#selected ? style.bold(message) : message}`);
        lines.push(`    ${role.meta(`revision ${boundary.surface_revision} · ${index + 1}/${visible.length}`)}`);
      });
    }
    lines.push("", role.meta("↑↓ navigate · Enter select · Esc close"));
    return lines.map((line) => truncateToWidth(line, width, "…"));
  }

  #move(delta: number, length: number): void {
    if (length === 0) return;
    this.#selected = (this.#selected + delta + length) % length;
    this.onChange?.();
  }
}

function sessionSearchText(session: SessionSummaryView): string {
  return `${session.name} ${session.id} ${session.active_node}`;
}

function boundarySearchText(boundary: SessionUserMessageBoundaryView): string {
  return `${boundary.message.id} ${boundary.surface_revision} ${previewUserMessage(boundary)}`;
}

function previewUserMessage(boundary: SessionUserMessageBoundaryView): string {
  return boundary.message.content
    .map((block) => (block.type === "text" ? block.text : `[${block.type}]`))
    .join(" ")
    .replace(/\s+/g, " ")
    .trim() || "(empty content)";
}

function isPrintable(data: string): boolean {
  return data.length === 1 && data >= " " && data !== "\u007f";
}
