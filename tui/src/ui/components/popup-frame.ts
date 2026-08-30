/**
 * The one shared frame for rustX's transient overlay surfaces.
 *
 * Every surface that takes interaction focus above the main surface — the
 * questionnaire, the model/session/boundary/tree pickers, the read-only
 * inspection dialogs (`/status`, `/help`, …) — is a {@link PopupContent}
 * wrapped in this frame by the app. The frame owns everything about the
 * popup's *geometry*; the wrapped component owns everything about its
 * *feature* (focus movement, filtering, scrolling, submission).
 *
 * The invariant this class enforces:
 *
 * > The popup's complete interactive region is visually contained within one
 * > unambiguous boundary, and no child section ever lays itself out relative
 * > to the terminal.
 *
 * Concretely, one finite layout root per popup:
 *
 * ```text
 * 1. the outer rectangle is the width pi-tui allocated, capped at the height
 *    budget the owning app derived from the terminal rows
 * 2. the surface background fills that rectangle edge to edge
 * 3. one outer border is drawn on all four sides, title integrated into the
 *    top edge
 * 4. the inner rectangle excludes the border and one cell of horizontal
 *    padding
 * 5. the body is rendered into the remaining finite rectangle — components
 *    derive their visible layout from it (see {@link PopupContent.setBodyHeight});
 *    the frame's own clipping below is only defensive containment, never the
 *    viewport implementation
 * 6. the footer/help lines sit inside the same boundary, below a separator
 * ```
 *
 * The wrapped component never sees the border cells: it is rendered at the
 * inner content width and receives the exact number of body rows the frame
 * allocated, so scrolling content cannot push the bottom border or the
 * footer off the surface, and a selection can never move into rows the frame
 * would have clipped away.
 *
 * Constrained terminals degrade gracefully and in one place: horizontal
 * padding yields first, then vertical padding, then the footer, and the body
 * is clipped to whatever finite rectangle remains. No popup can emit a line
 * wider than its outer rectangle or more rows than its budget, at any
 * terminal size.
 */

import {
  truncateToWidth,
  visibleWidth,
  type Component,
} from "@earendil-works/pi-tui";

import { background, fillBand, role } from "../theme.ts";

/** Horizontal cells between the frame's side borders and the content. */
const PAD_X = 1;
/** Vertical blank rows between the frame's top/bottom edges and the body. */
const PAD_Y = 1;

/**
 * A component that can live inside a {@link PopupFrame}.
 *
 * The component declares the popup's title and help/footer content — those
 * are facts about its keybindings and purpose — and renders only its body:
 * the frame owns where the title, the boundary, and the footer go. Feature
 * state machines (navigation, filtering, selection, submission) stay
 * entirely inside the component.
 */
export interface PopupContent extends Component {
  /** The popup's title, integrated into the frame's top border. */
  popupTitle(): string;
  /** Help/hint lines rendered inside the boundary, below a separator. */
  popupFooter(): string[];
  /**
   * The finite-layout hook every popup body must implement.
   *
   * The frame calls this with the exact number of body rows allocated for
   * the current render pass — after border, padding, and footer are
   * subtracted — before every `render`. The component must derive its
   * visible layout from this budget in physical rendered rows: headers and
   * interactive input first, the currently selected item always, surrounding
   * items as space permits, and subordinate metadata only with what remains.
   * A selection the user can move to must never fall outside the rows the
   * component returns.
   */
  setBodyHeight(height: number): void;
}

export class PopupFrame implements Component {
  readonly #content: PopupContent;
  #viewportHeight = 24;

  constructor(content: PopupContent) {
    this.#content = content;
  }

  /** The body component this frame presents. */
  get content(): PopupContent {
    return this.#content;
  }

  /**
   * Sets the outer height budget in rows, borders included.
   *
   * The owning app derives this from the terminal rows at the same percentage
   * it declares as the overlay's `maxHeight`, so pi-tui's own clipping never
   * has to slice the frame — the bottom border always renders.
   */
  setViewportHeight(height: number): void {
    this.#viewportHeight = Math.max(1, Math.floor(height));
  }

  get focused(): boolean {
    return (this.#content as { focused?: boolean }).focused ?? false;
  }

  set focused(value: boolean) {
    // pi-tui focuses the frame; the visible focus state (cursor markers,
    // highlighted search input) belongs to the wrapped component.
    (this.#content as { focused?: boolean }).focused = value;
  }

  handleInput(data: string): void {
    this.#content.handleInput?.(data);
  }

  invalidate(): void {
    this.#content.invalidate?.();
  }

  render(width: number): string[] {
    const frameWidth = Math.max(1, Math.floor(width));
    const budget = Math.max(1, this.#viewportHeight);

    // A surface too narrow for an interior draws only its boundary.
    if (frameWidth <= 2) {
      const glyph = (open: string, close: string) =>
        role.chrome(frameWidth === 1 ? open : `${open}${close}`);
      const rows = [glyph("╭", "╮")];
      while (rows.length < budget - 1) rows.push(glyph("│", "│"));
      if (budget > 1) rows.push(glyph("╰", "╯"));
      return rows.map((line) => fillBand(background.popup, line, frameWidth));
    }

    const padX = frameWidth >= 2 * PAD_X + 3 ? PAD_X : 0;
    const contentWidth = Math.max(1, frameWidth - 2 - 2 * padX);

    // Finite vertical allocation. The budget shrinks gracefully: vertical
    // padding yields first (below a minimally useful body), then the footer
    // block, and the body takes whatever rows remain.
    let padY = PAD_Y;
    let footer = this.#content.popupFooter().filter((line) => line.length > 0);
    const footerRows = () => (footer.length > 0 ? footer.length + 1 : 0);
    const interior = budget - 2;
    let bodyHeight = interior - 2 * padY - footerRows();
    if (bodyHeight < 3) {
      padY = 0;
      bodyHeight = interior - footerRows();
    }
    if (bodyHeight < 1 && footer.length > 0) {
      footer = [];
      bodyHeight = interior;
    }
    bodyHeight = Math.max(0, bodyHeight);

    let bodyLines: string[] = [];
    if (bodyHeight > 0) {
      // The one semantic path: the body lays itself out inside the finite
      // rectangle it is handed. The slice below is defensive containment
      // against a misbehaving body, not a viewport mechanism.
      this.#content.setBodyHeight(bodyHeight);
      bodyLines = this.#content
        .render(contentWidth)
        .slice(0, bodyHeight)
        .map((line) => truncateToWidth(line, contentWidth, "…"));
    }

    const row = (line: string): string =>
      `${role.chrome("│")}${" ".repeat(padX)}${pad(line, contentWidth)}${" ".repeat(padX)}${role.chrome("│")}`;

    const lines: string[] = [this.#topBorder(frameWidth)];
    for (let index = 0; index < padY; index += 1) lines.push(row(""));
    for (const line of bodyLines) lines.push(row(line));
    for (let index = 0; index < padY; index += 1) lines.push(row(""));
    if (footer.length > 0) {
      lines.push(role.chrome(`├${dash(frameWidth - 2)}┤`));
      for (const line of footer) {
        lines.push(row(truncateToWidth(role.meta(line), contentWidth, "…")));
      }
    }
    lines.push(role.chrome(`╰${dash(frameWidth - 2)}╯`));

    return lines
      .slice(0, budget)
      .map((line) => fillBand(background.popup, line, frameWidth));
  }

  /** The top edge, with the title integrated between the corner glyphs. */
  #topBorder(frameWidth: number): string {
    const title = this.#content.popupTitle().trim();
    // `╭─ ` + title + ` ─╮`: the title needs at least one trailing dash.
    const titleWidth = Math.min(visibleWidth(title), frameWidth - 7);
    if (titleWidth <= 0) {
      return role.chrome(`╭${dash(frameWidth - 2)}╮`);
    }
    const fitted = truncateToWidth(title, titleWidth, "…");
    const fill = frameWidth - 2 - 3 - visibleWidth(fitted);
    return (
      role.chrome("╭─ ") +
      role.strong(fitted) +
      role.chrome(` ${dash(fill)}╮`)
    );
  }
}

function dash(count: number): string {
  return "─".repeat(Math.max(0, count));
}

function pad(line: string, width: number): string {
  return line + " ".repeat(Math.max(0, width - visibleWidth(line)));
}

/**
 * A physical-row window into a popup list, anchored on the selected item.
 *
 * Popup bodies derive their visible viewport from the finite row budget the
 * PopupFrame allocated, in *physical rendered rows* — one item may cost
 * several rows (a selected row with details, a session row with metadata).
 * The selected index is always inside the returned window, even when its own
 * cost exceeds the budget (the caller then renders the entry's leading rows,
 * which carry the selection marker). The window grows downward first, then
 * upward, so the reading direction is preferred when both fit partially.
 *
 * This is geometry over abstract row costs only; it knows nothing about any
 * selector's items, and it never changes the logical selection.
 */
export function windowAroundSelected(
  count: number,
  selected: number,
  budget: number,
  rowCost: (index: number) => number,
): { start: number; end: number } {
  if (count === 0) return { start: 0, end: 0 };
  const anchor = Math.max(0, Math.min(selected, count - 1));
  let start = anchor;
  let end = anchor + 1;
  let used = rowCost(anchor);
  for (;;) {
    if (end < count && used + rowCost(end) <= budget) {
      used += rowCost(end);
      end += 1;
      continue;
    }
    if (start > 0 && used + rowCost(start - 1) <= budget) {
      used += rowCost(start - 1);
      start -= 1;
      continue;
    }
    break;
  }
  return { start, end };
}
