/**
 * One nonblocking status line for newly observed configuration repairs
 * (Issue #464).
 *
 * Source repairs belong to configuration sources, not to the Session on
 * screen, so a focus change does not retire this line; the first user input
 * acknowledges it, exactly like transient feedback. It is never a permanent
 * indicator and never part of the conversation: `/settings repairs` reads the
 * authoritative receipts again whenever the details are wanted.
 *
 * The line belongs to one App Server authority: it holds only unacknowledged
 * receipts that authority announced. Reconnecting to the same authority keeps
 * them and announces none of its receipts twice; a replacement authority
 * retires them all — its own receipts, even with equal content-derived ids,
 * are new announcements — and an observation of any other authority, such
 * as a late notice from a replaced connection, changes nothing.
 */

import { truncateToWidth, wrapTextWithAnsi, type Component } from "@earendil-works/pi-tui";

import type { ConfigRepairReceipt } from "../../protocol/app-server.ts";
import { SourceRepairNotices, repairNotice, sourceKey, type SourceRepairObservation } from "../../presentation/source-repairs.ts";
import { role } from "../theme.ts";

const MAX_ROWS = 2;

export class SourceRepairStatus implements Component {
  /** The App Server authority whose receipts this line presents. */
  #authorityId: string | undefined;
  /** Unacknowledged receipts of that authority, at most one per source. */
  #receipts = new Map<string, ConfigRepairReceipt>();
  /** Receipts already announced, by authority and receipt id. */
  readonly #notices = new SourceRepairNotices();

  /** Presents `authorityId` from now on. A different authority retires every
   * receipt of the previous one; the same authority keeps them. */
  adopt(authorityId: string): void {
    if (authorityId !== this.#authorityId) this.#receipts.clear();
    this.#authorityId = authorityId;
  }

  /**
   * Folds one native observation of `authorityId` in: a receipt not yet
   * announced replaces its source's line entry, and an observation without a
   * receipt withdraws that source. Returns whether the line changed.
   */
  observe(authorityId: string, { target, receipt }: SourceRepairObservation): boolean {
    if (authorityId !== this.#authorityId) return false;
    if (receipt === undefined) return this.#receipts.delete(sourceKey(target));
    if (!this.#notices.fresh(authorityId, receipt)) return false;
    this.#receipts.set(sourceKey(target), receipt);
    return true;
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
