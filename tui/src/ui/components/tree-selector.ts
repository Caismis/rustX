/** Searchable native Session graph selector. */

import {
  fuzzyMatch,
  matchesKey,
  truncateToWidth,
  type Component,
  type Focusable,
} from "@earendil-works/pi-tui";
import type {
  SessionNodeView,
  SessionUserMessageBoundaryView,
  SessionView,
} from "../../protocol/types.ts";
import { role, style } from "../theme.ts";

export type TreeSelection =
  | { kind: "node"; node: SessionNodeView }
  | { kind: "branch"; boundary: SessionUserMessageBoundaryView };

export interface TreeSelectorOptions {
  session: SessionView;
  nodes: SessionNodeView[];
  nextNodeOffset?: number;
  boundaries: SessionUserMessageBoundaryView[];
  nextHistoryOffset?: number;
}

export class TreeSelector implements Component, Focusable {
  focused = false;
  onSelect?: (selection: TreeSelection) => void;
  onCancel?: () => void;
  onChange?: () => void;
  onLoadMore?: () => void;

  readonly #session: SessionView;
  #nodes: SessionNodeView[];
  #nextNodeOffset: number | undefined;
  #boundaries: SessionUserMessageBoundaryView[];
  #nextHistoryOffset: number | undefined;
  #query = "";
  #selected = 0;
  #loading = false;

  constructor(options: TreeSelectorOptions) {
    this.#session = options.session;
    this.#nodes = options.nodes;
    this.#nextNodeOffset = options.nextNodeOffset;
    this.#boundaries = options.boundaries;
    this.#nextHistoryOffset = options.nextHistoryOffset;
    this.#selected = Math.max(0, this.items().length - 1);
  }

  /** Appends bounded native node/history pages after a continuation request. */
  appendPage(options: {
    nodes: SessionNodeView[];
    nextNodeOffset?: number;
    boundaries: SessionUserMessageBoundaryView[];
    nextHistoryOffset?: number;
  }): void {
    this.#nodes = [...this.#nodes, ...options.nodes];
    this.#nextNodeOffset = options.nextNodeOffset;
    this.#boundaries = [...this.#boundaries, ...options.boundaries];
    this.#nextHistoryOffset = options.nextHistoryOffset;
    this.#loading = false;
    this.onChange?.();
  }

  invalidate(): void {
    // The selector has no cached render state.
  }

  private items(): TreeSelection[] {
    const items: TreeSelection[] = this.#nodes.map((node) => ({
      kind: "node",
      node,
    }));
    items.push(
      ...this.#boundaries.map((boundary): TreeSelection => ({
        kind: "branch",
        boundary,
      })),
    );
    if (this.#query.length === 0) return items;
    return items.filter((item) => fuzzyMatch(this.#query, searchText(item)).matches);
  }

  handleInput(data: string): void {
    const visible = this.items();
    if (matchesKey(data, "escape")) this.onCancel?.();
    else if (matchesKey(data, "enter")) {
      const selected = visible[this.#selected];
      if (selected !== undefined) this.onSelect?.(selected);
    } else if (matchesKey(data, "up")) this.#move(-1, visible.length);
    else if (matchesKey(data, "down")) {
      if (
        this.#selected >= visible.length - 1 &&
        (this.#nextNodeOffset !== undefined || this.#nextHistoryOffset !== undefined) &&
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
    const visible = this.items();
    const lines = [
      role.strong("Session tree"),
      role.meta("Select a node to activate it, or a user boundary to create a new node."),
      `${role.meta("Search:")} ${this.#query}${this.focused ? role.accent("▌") : ""}`,
      "",
    ];
    if (visible.length === 0) {
      lines.push(role.meta("no tree item matches the search"));
    } else {
      const start = Math.max(0, Math.min(this.#selected - 3, visible.length - 8));
      visible.slice(start, start + 8).forEach((item, offset) => {
        const index = start + offset;
        const marker = index === this.#selected ? role.accent("❯") : " ";
        if (item.kind === "node") {
          const active = item.node.id === this.#session.active_node ? role.success(" active") : "";
          const prefix = item.node.parent === undefined ? "├─" : "└─";
          lines.push(`${marker} ${prefix} ${index === this.#selected ? style.bold(item.node.id) : item.node.id}${active}`);
          lines.push(`      ${role.meta(`${item.node.conversation_id} · ${originLabel(item.node.origin.type)}`)}`);
        } else {
          lines.push(`${marker} ${style.italic(`branch at: ${preview(item.boundary)}`)}`);
          lines.push(`      ${role.meta(`revision ${item.boundary.surface_revision}`)}`);
        }
      });
      if (this.#nextNodeOffset !== undefined || this.#nextHistoryOffset !== undefined) {
        lines.push(role.meta("↓ load more native tree/history rows"));
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

function searchText(item: TreeSelection): string {
  return item.kind === "node"
    ? `${item.node.id} ${item.node.conversation_id} ${item.node.origin.type}`
    : `${item.boundary.message.id} ${item.boundary.surface_revision} ${preview(item.boundary)}`;
}

function preview(boundary: SessionUserMessageBoundaryView): string {
  return boundary.message.content
    .map((block) => block.type === "text" ? block.text : `[${block.type}]`)
    .join(" ")
    .replace(/\s+/g, " ")
    .trim() || "(empty content)";
}

function originLabel(origin: string): string {
  return origin === "new" ? "root" : origin;
}

function isPrintable(data: string): boolean {
  return data.length === 1 && data >= " " && data !== "\u007f";
}
