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
import { sessionRowLabel } from "../../presentation/selectors.ts";
import { role, style } from "../theme.ts";

const VISIBLE_ROWS = 8;

export interface SessionSelectorOptions {
  sessions: SessionSummaryView[];
  nextOffset?: number;
  query?: string;
}

export class SessionSelector implements Component, Focusable {
  focused = false;
  onSelect?: (session: SessionSummaryView) => void;
  onCancel?: () => void;
  onChange?: () => void;
  onQueryChange?: (query: string) => void;
  onLoadMore?: () => void;

  #sessions: SessionSummaryView[];
  #nextOffset: number | undefined;
  #query: string;
  #selected = 0;
  #loading = false;

  constructor(options: SessionSelectorOptions) {
    this.#sessions = options.sessions;
    this.#nextOffset = options.nextOffset;
    this.#query = options.query ?? "";
  }

  /** Replaces the current native page after a query change. */
  replacePage(sessions: SessionSummaryView[], nextOffset?: number): void {
    this.#sessions = sessions;
    this.#nextOffset = nextOffset;
    this.#selected = 0;
    this.#loading = false;
    this.onChange?.();
  }

  /** Appends one native continuation page. */
  appendPage(sessions: SessionSummaryView[], nextOffset?: number): void {
    this.#sessions = [...this.#sessions, ...sessions];
    this.#nextOffset = nextOffset;
    this.#loading = false;
    this.onChange?.();
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
      if (
        this.#selected >= visible.length - 1 &&
        this.#nextOffset !== undefined &&
        !this.#loading
      ) {
        this.#loading = true;
        this.onLoadMore?.();
      }
      this.#move(1, visible.length);
    } else if (matchesKey(data, "backspace")) {
      this.#query = this.#query.slice(0, -1);
      this.#selected = 0;
      this.onQueryChange?.(this.#query);
      this.onChange?.();
    } else if (isPrintable(data)) {
      this.#query += data;
      this.#selected = 0;
      this.onQueryChange?.(this.#query);
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
        const label = sessionRowLabel(session);
        lines.push(`${marker} ${index === this.#selected ? style.bold(label) : label}${active}`);
        lines.push(`    ${role.meta(`${session.id} · node ${session.active_node}`)}`);
      }
      if (visible.length > VISIBLE_ROWS) {
        lines.push(role.meta(`${this.#selected + 1}/${visible.length}`));
      }
      if (this.#nextOffset !== undefined) {
        lines.push(role.meta("↓ load more matching Sessions"));
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
  nextOffset?: number;
}

export class BoundarySelector implements Component, Focusable {
  focused = false;
  onSelect?: (boundary: SessionUserMessageBoundaryView) => void;
  onCancel?: () => void;
  onChange?: () => void;
  onLoadMore?: () => void;

  #boundaries: SessionUserMessageBoundaryView[];
  #nextOffset: number | undefined;
  readonly #title: string;
  #query = "";
  #selected = 0;
  #loading = false;

  constructor(options: BoundarySelectorOptions) {
    this.#boundaries = options.boundaries;
    this.#nextOffset = options.nextOffset;
    this.#title = options.title;
    this.#selected = Math.max(0, this.#boundaries.length - 1);
  }

  invalidate(): void {
    // The selector has no cached render state.
  }

  /** Appends one bounded native history page. */
  appendPage(
    boundaries: SessionUserMessageBoundaryView[],
    nextOffset?: number,
  ): void {
    this.#boundaries = [...this.#boundaries, ...boundaries];
    this.#nextOffset = nextOffset;
    this.#loading = false;
    this.onChange?.();
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
    else if (matchesKey(data, "down")) {
      if (
        this.#selected >= visible.length - 1 &&
        this.#nextOffset !== undefined &&
        !this.#loading
      ) {
        this.#loading = true;
        this.onLoadMore?.();
      }
      this.#move(1, visible.length);
    }
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
      if (this.#nextOffset !== undefined) {
        lines.push(role.meta("↓ load more committed boundaries"));
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

// A row is searched by everything it can be recognized by, which for an
// unnamed Session is the first message it shows. Rust filters the same way,
// so a query narrows the same rows whether it is answered from the current
// page or from the catalog.
function sessionSearchText(session: SessionSummaryView): string {
  return `${sessionRowLabel(session)} ${session.id} ${session.active_node}`;
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
