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
import {
  windowAroundSelected,
  type PopupContent,
} from "./popup-frame.ts";

/** Default body rows when rendered without a frame (component tests). */
const DEFAULT_BODY_HEIGHT = 24;

/** Physical rows one tree entry costs: the item row and its metadata row. */
const TREE_ENTRY_ROWS = 2;

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
  #bodyHeight = DEFAULT_BODY_HEIGHT;

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

  /**
   * The finite body-row budget the PopupFrame allocated for this pass. The
   * list viewport below is derived from it in physical rendered rows — two
   * per node/branch entry — so the selected entry can never scroll into rows
   * the frame would clip. Logical selection still spans the complete
   * filtered set; only the visible window moves.
   */
  setBodyHeight(height: number): void {
    this.#bodyHeight = Math.max(1, Math.floor(height));
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
    // The interactive header always renders first: hint, then the query row.
    const header = [
      role.meta("Select a node to activate it, or a user boundary to create a new node."),
      `${role.meta("Search:")} ${this.#query}${this.focused ? role.accent("▌") : ""}`,
      "",
    ];
    const budget = Math.max(1, this.#bodyHeight);
    const lines = [...header];
    if (visible.length === 0) {
      lines.push(role.meta("no tree item matches the search"));
      return lines
        .slice(0, budget)
        .map((line) => truncateToWidth(line, width, "…"));
    }

    // The list owns the first claim on the remaining body rows, anchored on
    // the selected entry; the load-more hint is subordinate and yields first
    // under constrained heights.
    const tail: string[] = [];
    if (this.#nodeCursor.state === "active" || this.#historyCursor.state === "active") {
      tail.push(role.meta("↓ load more native tree/history rows"));
    }
    let listBudget = budget - header.length - tail.length;
    while (listBudget < TREE_ENTRY_ROWS && tail.length > 0) {
      tail.pop();
      listBudget += 1;
    }
    const window = windowAroundSelected(
      visible.length,
      this.#selected,
      Math.max(1, listBudget),
      () => TREE_ENTRY_ROWS,
    );
    for (let index = window.start; index < window.end; index += 1) {
      const item = visible[index]!;
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
    }
    lines.push(...tail.slice(0, Math.max(0, budget - lines.length)));
    return lines
      .slice(0, budget)
      .map((line) => truncateToWidth(line, width, "…"));
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
