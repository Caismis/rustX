import type { ConfigRepairReceipt } from '../../../protocol/app-server/v46';
import type { SourceRepairObservation } from '../client/app-server';

/** Which native repair receipts this browser has already acknowledged.
 *
 * A notification announces one native receipt identity once: a reconnect, a
 * second tab, a page reload or the same receipt delivered again re-presents
 * nothing the user already dismissed or opened. Only opaque receipt ids are
 * kept — never a path, change or diagnostic — and the list is finite. */
export class RepairAcknowledgements {
  static readonly KEY = 'rustx.configuration-repairs.acknowledged';
  static readonly LIMIT = 64;
  readonly #storage: Pick<Storage, 'getItem' | 'setItem'> | undefined;
  #ids: string[];

  constructor(storage: Pick<Storage, 'getItem' | 'setItem'> | undefined = browserStorage()) {
    this.#storage = storage;
    let ids: unknown;
    try { ids = JSON.parse(storage?.getItem(RepairAcknowledgements.KEY) ?? '[]'); } catch { ids = []; }
    this.#ids = Array.isArray(ids) ? ids.filter((id): id is string => typeof id === 'string').slice(-RepairAcknowledgements.LIMIT) : [];
  }

  has(id: string): boolean {
    return this.#ids.includes(id);
  }

  acknowledge(ids: readonly string[]): void {
    for (const id of ids) if (!this.#ids.includes(id)) this.#ids.push(id);
    this.#ids = this.#ids.slice(-RepairAcknowledgements.LIMIT);
    try { this.#storage?.setItem(RepairAcknowledgements.KEY, JSON.stringify(this.#ids)); } catch { /* storage is a convenience only */ }
  }
}

/** The browser's persistent storage, when this document may use it. Reading
 * `localStorage` itself throws a `SecurityError` where storage is denied; the
 * acknowledgements then live in memory for this page only. */
function browserStorage(): Storage | undefined {
  try { return globalThis.localStorage; } catch { return undefined; }
}

/** The receipts a notification presents now: each source's latest receipt
 * this browser has not acknowledged. A healthy source has no receipt at all,
 * so it is never presented. */
export function unacknowledgedRepairs(observations: Readonly<Record<string, SourceRepairObservation>> | undefined, acknowledged: RepairAcknowledgements): ConfigRepairReceipt[] {
  return Object.values(observations ?? {})
    .map(observation => observation.receipt)
    .filter((receipt): receipt is ConfigRepairReceipt => receipt !== undefined && !acknowledged.has(receipt.id))
    // User first, then Workspaces: a stable, owner-ordered presentation.
    .sort((a, b) => (a.target.kind === 'user' ? 0 : 1) - (b.target.kind === 'user' ? 0 : 1));
}
