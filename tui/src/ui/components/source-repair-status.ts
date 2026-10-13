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
 * The line belongs to one App Server authority. Three facts stay apart: each
 * source's current native observation (ordered by native ledger sequence,
 * never by receipt id or arrival), the receipts already announced (once per
 * authority and receipt id), and what the line presents — a source's current
 * receipt from its first announcement until the user acknowledges it or a
 * newer observation replaces or withdraws it. A newer observation therefore
 * always updates its source, even when its receipt was announced before:
 * such a receipt becomes current without being announced again, and retires
 * whatever the source presented. An acknowledgement never changes what a
 * source holds.
 *
 * The line orders what the host admitted. The host never admits an
 * observation of a source older than one it already delivered, even after
 * forgetting that source, and a later connection to the same authority
 * observes each source no older than its predecessor delivered it; so when
 * the line forgets a source of its own (64 Workspaces and the User, the
 * least recently observed Workspace first), nothing it later admits for
 * that source is stale.
 *
 * Reconnecting to the same authority keeps the entries and the sequences they
 * are current at; the caller rereads the sources still presented, because a
 * missing notice proves nothing (a connection's first observation of a source
 * without a receipt is never announced). A replacement authority retires
 * everything — its sequences are not comparable — and an observation of any
 * other authority changes nothing.
 */

import { truncateToWidth, wrapTextWithAnsi, type Component } from "@earendil-works/pi-tui";

import type { ConfigRepairReceipt, SourceTarget } from "../../protocol/app-server.ts";
import { SourceRepairAnnouncements, olderSequence, repairNotice, retainSource, sourceKey, type SourceRepairObservation } from "../../presentation/source-repairs.ts";
import { role } from "../theme.ts";

const MAX_ROWS = 2;

/** One source's current native observation, and what of it is presented. */
interface SourceState {
  target: SourceTarget;
  /** The native ledger sequence of the newest observation applied. */
  sequence: string;
  /** Its receipt while presented: announced and not yet acknowledged. */
  pending: ConfigRepairReceipt | undefined;
}

export class SourceRepairStatus implements Component {
  /** The App Server authority whose observations this line presents. */
  #authorityId: string | undefined;
  /** Per source, least recently observed first; finite (`retainSource`). */
  #sources = new Map<string, SourceState>();
  readonly #announced = new SourceRepairAnnouncements();

  /**
   * Presents `authorityId` from now on. A different authority retires every
   * source of the previous one. Returns whether the same authority's entries
   * were kept, so the caller reconciles them against native authority.
   */
  adopt(authorityId: string): boolean {
    const kept = authorityId === this.#authorityId;
    if (!kept) this.#sources.clear();
    this.#authorityId = authorityId;
    return kept;
  }

  /**
   * Applies one native observation of `authorityId`, unless it is older than
   * the source's current one. The source's presented receipt stays while it
   * is still the current receipt; a receipt new to the source is presented
   * only on its first announcement; no receipt withdraws the source. Returns
   * whether the line changed.
   */
  observe(authorityId: string, { target, receipt, sequence }: SourceRepairObservation): boolean {
    if (authorityId !== this.#authorityId) return false;
    const key = sourceKey(target);
    const known = this.#sources.get(key);
    if (known !== undefined && olderSequence(sequence, known.sequence)) return false;
    const pending = receipt === undefined ? undefined
      : known?.pending?.id === receipt.id ? known.pending
      : this.#announced.announce(authorityId, receipt) ? receipt : undefined;
    const forgotten = retainSource(this.#sources, key, { target, sequence, pending });
    return known?.pending?.id !== pending?.id || forgotten?.pending !== undefined;
  }

  /** The sources whose receipts the line still presents. */
  pendingSources(): SourceTarget[] {
    return [...this.#sources.values()].filter(source => source.pending !== undefined).map(source => source.target);
  }

  get visible(): boolean {
    return this.pendingSources().length > 0;
  }

  /** A user action acknowledges every receipt on the line. */
  acknowledge(): void {
    for (const source of this.#sources.values()) source.pending = undefined;
  }

  render(width: number): string[] {
    const notice = repairNotice([...this.#sources.values()].flatMap(source => source.pending ? [source.pending] : []));
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
