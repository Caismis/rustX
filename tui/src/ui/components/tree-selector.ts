/** Searchable native Session graph selector. */

import {
  fuzzyMatch,
  matchesKey,
  truncateToWidth,
  type Focusable,
} from "@earendil-works/pi-tui";
import type {
  SessionNodeView,
  SessionUserMessageBoundaryView,
  SessionView,
} from "../../protocol/types.ts";
import { role, style } from "../theme.ts";
import type { PopupContent } from "./popup-frame.ts";

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

export interface TreePageRequest {
  nodeOffset: number;
  historyOffset: number;
}

type PageCursor =
  | { state: "active"; offset: number }
  | { state: "exhausted" };

function pageCursor(offset: number | undefined): PageCursor {
  return offset === undefined ? { state: "exhausted" } : { state: "active", offset };
}

export class TreeSelector implements PopupContent, Focusable {
  focused = false;
  onSelect?: (selection: TreeSelection) => void;
  onCancel?: () => void;
  onChange?: () => void;
  onLoadMore?: () => void;

  readonly #session: SessionView;
  #nodes: SessionNodeView[];
  #nodeCursor: PageCursor;
  #boundaries: SessionUserMessageBoundaryView[];
  #historyCursor: PageCursor;
  #query = "";
  #selected = 0;
  #loading = false;

  constructor(options: TreeSelectorOptions) {
    this.#session = options.session;
    this.#nodes = options.nodes;
    this.#nodeCursor = pageCursor(options.nextNodeOffset);
    this.#boundaries = options.boundaries;
    this.#historyCursor = pageCursor(options.nextHistoryOffset);
    this.#selected = Math.max(0, this.items().length - 1);
  }

  /**
   * Returns the next paired native request without reviving an exhausted
   * stream. The native endpoint pages nodes and historical boundaries
   * independently in one response, so an exhausted side advances at its
   * current materialized length as a monotonic no-op while the other side
   * continues.
   */
  nextPageRequest(): TreePageRequest | undefined {
    if (this.#nodeCursor.state === "exhausted" && this.#historyCursor.state === "exhausted") {
      return undefined;
    }
    return {
      nodeOffset: this.#nodeCursor.state === "active"
        ? this.#nodeCursor.offset
        : this.#nodes.length,
      historyOffset: this.#historyCursor.state === "active"
        ? this.#historyCursor.offset
        : this.#boundaries.length,
    };
  }

  /** Appends bounded native node/history pages after a continuation request. */
  appendPage(options: {
    nodes: SessionNodeView[];
    nextNodeOffset?: number;
    boundaries: SessionUserMessageBoundaryView[];
    nextHistoryOffset?: number;
  }): void {
    this.#nodes = [...this.#nodes, ...options.nodes];
    this.#nodeCursor = pageCursor(options.nextNodeOffset);
    this.#boundaries = [...this.#boundaries, ...options.boundaries];
    this.#historyCursor = pageCursor(options.nextHistoryOffset);
    this.#loading = false;
    this.onChange?.();
  }

  /** Releases the load guard after a failed page request for a retry. */
  retryPage(): void {
    this.#loading = false;
    this.onChange?.();
  }

  invalidate(): void {
    // The selector has no cached render state.
  }

  /** The popup's frame title. */
  popupTitle(): string {
    return "Session tree";
  }

  /** The popup's help line, contained by the frame below the body. */
  popupFooter(): string[] {
    return ["↑↓ navigate · Enter select · Esc close"];
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
        (this.#nodeCursor.state === "active" || this.#historyCursor.state === "active") &&
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
      if (this.#nodeCursor.state === "active" || this.#historyCursor.state === "active") {
        lines.push(role.meta("↓ load more native tree/history rows"));
      }
    }
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
