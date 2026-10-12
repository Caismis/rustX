/**
 * One nonblocking status line for newly observed configuration repairs
 * (Issue #464).
 *
 * Source repairs belong to configuration sources, not to the Session on
 * screen, so a focus change does not retire this line; the first user input
 * acknowledges it, exactly like transient feedback. It is never a permanent
 * indicator and never part of the conversation: `/settings repairs` reads the
 * authoritative receipts again whenever the details are wanted.
 */

import { truncateToWidth, wrapTextWithAnsi, type Component } from "@earendil-works/pi-tui";

import type { ConfigRepairReceipt } from "../../protocol/app-server.ts";
import { repairNotice, sourceKey } from "../../presentation/source-repairs.ts";
import { role } from "../theme.ts";

const MAX_ROWS = 2;

export class SourceRepairStatus implements Component {
  /** Unacknowledged receipts, at most one per source. */
  #receipts = new Map<string, ConfigRepairReceipt>();

  /** Adds one newly observed receipt; a later receipt of a source replaces it. */
  observe(receipt: ConfigRepairReceipt): void {
    this.#receipts.set(sourceKey(receipt.target), receipt);
  }

  /** Withdraws a source whose repair observation was cleared natively. */
  withdraw(key: string): void {
    this.#receipts.delete(key);
  }

  get visible(): boolean {
    return this.#receipts.size > 0;
  }

  /** A user action acknowledges the line. */
  acknowledge(): void {
    this.#receipts.clear();
  }

  render(width: number): string[] {
    const notice = repairNotice([...this.#receipts.values()]);
    if (notice === undefined) return [];
    const color = notice.level === "error" ? role.error : notice.level === "warning" ? role.warning : role.meta;
    const rows = wrapTextWithAnsi(color(notice.text), Math.max(1, width));
    // Bounded: the details are one /settings repairs away.
    if (rows.length > MAX_ROWS) rows.splice(MAX_ROWS - 1, rows.length, truncateToWidth(rows.slice(MAX_ROWS - 1).join(" "), Math.max(1, width), "…"));
    return rows;
  }

  invalidate(): void {
    // Rendering derives directly from the unacknowledged receipts.
  }
}
