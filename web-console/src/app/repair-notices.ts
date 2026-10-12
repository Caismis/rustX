import type { ConfigRepairReceipt } from '../../../protocol/app-server/v46';
import type { SourceRepairObservation } from '../client/app-server';

/** One native receipt as the App Server authority that produced it announced
 * it. A receipt id is content-derived, so only the pair identifies a repair. */
export interface PresentedRepair {
  authorityId: string;
  receipt: ConfigRepairReceipt;
}

type Acknowledgement = readonly [authorityId: string, receiptId: string];

/** Which native repair receipts this browser has already acknowledged, per
 * App Server authority.
 *
 * A notification announces one receipt of one authority once: a reconnect, a
 * second tab, a page reload or the same receipt delivered again by that
 * authority re-presents nothing the user already dismissed or opened, while an
 * equal receipt id from another authority — another App Server, or a new
 * process at the same endpoint — is a repair of its own. Only opaque
 * identities are kept — never a path, change or diagnostic — and the list is
 * finite. */
export class RepairAcknowledgements {
  static readonly KEY = 'rustx.configuration-repairs.acknowledged';
  static readonly LIMIT = 64;
  readonly #storage: Pick<Storage, 'getItem' | 'setItem'> | undefined;
  #acknowledged: Acknowledgement[];

  constructor(storage: Pick<Storage, 'getItem' | 'setItem'> | undefined = browserStorage()) {
    this.#storage = storage;
    let stored: unknown;
    try { stored = JSON.parse(storage?.getItem(RepairAcknowledgements.KEY) ?? '[]'); } catch { stored = []; }
    this.#acknowledged = Array.isArray(stored)
      ? stored.filter((entry): entry is Acknowledgement => Array.isArray(entry) && entry.length === 2 && entry.every(part => typeof part === 'string')).slice(-RepairAcknowledgements.LIMIT)
      : [];
  }

  has({ authorityId, receipt }: PresentedRepair): boolean {
    return this.#acknowledged.some(([authority, id]) => authority === authorityId && id === receipt.id);
  }

  acknowledge(repairs: readonly PresentedRepair[]): void {
    for (const repair of repairs) if (!this.has(repair)) this.#acknowledged.push([repair.authorityId, repair.receipt.id]);
    this.#acknowledged = this.#acknowledged.slice(-RepairAcknowledgements.LIMIT);
    try { this.#storage?.setItem(RepairAcknowledgements.KEY, JSON.stringify(this.#acknowledged)); } catch { /* storage is a convenience only */ }
  }
}

/** The browser's persistent storage, when this document may use it. Reading
 * `localStorage` itself throws a `SecurityError` where storage is denied; the
 * acknowledgements then live in memory for this page only. */
function browserStorage(): Storage | undefined {
  try { return globalThis.localStorage; } catch { return undefined; }
}

/** The receipts a notification presents now: each source's latest receipt
 * this browser has not acknowledged for its authority. A healthy source has
 * no receipt at all, so it is never presented. */
export function unacknowledgedRepairs(observations: Readonly<Record<string, SourceRepairObservation>> | undefined, acknowledged: RepairAcknowledgements): PresentedRepair[] {
  return Object.values(observations ?? {})
    .flatMap(({ authorityId, receipt }) => receipt ? [{ authorityId, receipt }] : [])
    .filter(repair => !acknowledged.has(repair))
    // User first, then Workspaces: a stable, owner-ordered presentation.
    .sort((a, b) => (a.receipt.target.kind === 'user' ? 0 : 1) - (b.receipt.target.kind === 'user' ? 0 : 1));
}
