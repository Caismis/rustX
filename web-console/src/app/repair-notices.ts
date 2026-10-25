import type { ConfigRepairReceipt } from '../../../protocol/app-server/v57';
import type { AppServerClient } from '../client/app-server';
import type { SourceRepairObservation } from '../client/contracts';
import { SOURCE_REPAIR_WORKSPACES } from '../../../protocol/app-server/source-repair';

/** The identity of one repair: a receipt id names one repair occurrence
 * within the App Server authority that produced it, so the authority and the
 * id together identify it. */
export interface RepairIdentity {
  authorityId: string;
  receiptId: string;
}

/** One native receipt as the App Server authority that produced it announced
 * it. */
export interface PresentedRepair {
  authorityId: string;
  receipt: ConfigRepairReceipt;
}

/** How much a receipt matters to a combined notice: a publication whose
 * durability was not confirmed — reread before editing or retrying — outranks
 * a repair that published nothing, which outranks a committed one. */
const SEVERITY: Record<ConfigRepairReceipt['status'], number> = { repaired: 0, unresolved: 1, commit_uncertain: 2 };

/** What a combined notice is about: the item whose receipt is most severe,
 * the first of equals in the order given. */
export function mostSevere<T>(items: readonly T[], receipt: (item: T) => ConfigRepairReceipt): T | undefined {
  return items.reduce<T | undefined>((subject, item) =>
    subject === undefined || SEVERITY[receipt(item).status] > SEVERITY[receipt(subject).status] ? item : subject, undefined);
}

/** The identity of one presented repair. */
export function repairIdentity({ authorityId, receipt }: PresentedRepair): RepairIdentity {
  return { authorityId, receiptId: receipt.id };
}

/** Completes a View details inspection: acknowledges its receipt, and only
 * while the client is still connected to the authority that produced it. */
export function inspectionAcknowledger(client: Pick<AppServerClient, 'getSnapshot'>, acknowledgements: RepairAcknowledgements) {
  return ({ authorityId, receiptId }: RepairIdentity) => {
    if (client.getSnapshot().authorityId === authorityId) acknowledgements.acknowledge([{ authorityId, receiptId }]);
  };
}

type Acknowledgement = readonly [authorityId: string, receiptId: string];
type StorageEvents = Pick<EventTarget, 'addEventListener' | 'removeEventListener'>;

/** Which native repair receipts this browser has already acknowledged, per
 * App Server authority.
 *
 * A notification announces one receipt of one authority once: a reconnect, a
 * second tab, a page reload or the same receipt delivered again by that
 * authority re-presents nothing the user already dismissed or inspected,
 * while an equal receipt id from another authority — another App Server, or a
 * new process at the same endpoint — is a repair of its own. Only opaque
 * identities are kept — never a path, change or diagnostic — and the list is
 * finite.
 *
 * Tabs converge, without being linearizable. Every write first merges the
 * identities currently persisted, so a tab never overwrites another tab's
 * earlier acknowledgements with a stale snapshot; and a `storage` event from
 * another tab merges its acknowledgements here and re-renders subscribers.
 * `localStorage` has no compare-and-swap, so two tabs writing at the same
 * instant can still each persist a list without the other's newest entry; the
 * lost acknowledgement stays in its own tab's memory, is persisted again by
 * that tab's next write, and at worst presents one notice once more elsewhere.
 * Where storage is denied, acknowledgements live in this page's memory. */
export class RepairAcknowledgements {
  static readonly KEY = 'rustx.configuration-repairs.acknowledged';
  /** At least every receipt one notice can present at once — the User and
   * `SOURCE_REPAIR_WORKSPACES` Workspaces — so one Dismiss can never leave a
   * presented receipt unacknowledged, with room to keep earlier dismissals
   * across reconnects and reloads. */
  static readonly LIMIT = 4 * (SOURCE_REPAIR_WORKSPACES + 1);
  readonly #storage: Pick<Storage, 'getItem' | 'setItem'> | undefined;
  readonly #events: StorageEvents | undefined;
  readonly #listeners = new Set<() => void>();
  #acknowledged: Acknowledgement[];
  #version = 0;

  constructor(storage: Pick<Storage, 'getItem' | 'setItem'> | undefined = browserStorage(), events: StorageEvents | undefined = browserEvents()) {
    this.#storage = storage;
    this.#events = events;
    this.#acknowledged = this.#persisted();
  }

  has({ authorityId, receiptId }: RepairIdentity): boolean {
    return this.#acknowledged.some(([authority, id]) => authority === authorityId && id === receiptId);
  }

  /** Acknowledge `identities`, merged with what other tabs persisted. */
  acknowledge(identities: readonly RepairIdentity[]): void {
    const added = identities.map(({ authorityId, receiptId }): Acknowledgement => [authorityId, receiptId]);
    this.#acknowledged = merge(this.#persisted(), this.#acknowledged, added);
    try { this.#storage?.setItem(RepairAcknowledgements.KEY, JSON.stringify(this.#acknowledged)); } catch { /* storage is a convenience only */ }
    this.#changed();
  }

  /** Subscribe to changed acknowledgements, including another tab's. */
  readonly subscribe = (listener: () => void): (() => void) => {
    if (this.#listeners.size === 0) this.#events?.addEventListener('storage', this.#observe);
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
      if (this.#listeners.size === 0) this.#events?.removeEventListener('storage', this.#observe);
    };
  };

  /** Changes with every change of the acknowledged identities. */
  readonly version = (): number => this.#version;

  readonly #observe = (event: Event) => {
    const key = (event as StorageEvent).key;
    if (key !== RepairAcknowledgements.KEY && key !== null) return;
    const merged = merge(this.#persisted(), this.#acknowledged, []);
    if (merged.length === this.#acknowledged.length && merged.every(([authority, id], index) => authority === this.#acknowledged[index][0] && id === this.#acknowledged[index][1])) return;
    this.#acknowledged = merged;
    this.#changed();
  };

  #changed() {
    this.#version++;
    for (const listener of [...this.#listeners]) listener();
  }

  /** The persisted identities; entries of any other shape are ignored. */
  #persisted(): Acknowledgement[] {
    let stored: unknown;
    try { stored = JSON.parse(this.#storage?.getItem(RepairAcknowledgements.KEY) ?? '[]'); } catch { stored = []; }
    return Array.isArray(stored)
      ? stored.filter((entry): entry is Acknowledgement => Array.isArray(entry) && entry.length === 2 && entry.every(part => typeof part === 'string')).slice(-RepairAcknowledgements.LIMIT)
      : [];
  }
}

/** `persisted`, then this tab's identities it lacks, then `added` moved to the
 * end as the newest; bounded to the most recent `LIMIT`. */
function merge(persisted: readonly Acknowledgement[], memory: readonly Acknowledgement[], added: readonly Acknowledgement[]): Acknowledgement[] {
  const key = ([authority, id]: Acknowledgement) => JSON.stringify([authority, id]);
  const fresh = new Set(added.map(key));
  const result: Acknowledgement[] = [];
  const seen = new Set<string>();
  for (const entry of [...persisted, ...memory]) {
    const identity = key(entry);
    if (fresh.has(identity) || seen.has(identity)) continue;
    seen.add(identity);
    result.push(entry);
  }
  for (const entry of added) {
    const identity = key(entry);
    if (seen.has(identity)) continue;
    seen.add(identity);
    result.push(entry);
  }
  return result.slice(-RepairAcknowledgements.LIMIT);
}

/** The browser's persistent storage, when this document may use it. Reading
 * `localStorage` itself throws a `SecurityError` where storage is denied; the
 * acknowledgements then live in memory for this page only. */
function browserStorage(): Storage | undefined {
  try { return globalThis.localStorage; } catch { return undefined; }
}

/** Where this document receives other tabs' `storage` events, if anywhere. */
function browserEvents(): StorageEvents | undefined {
  return typeof globalThis.addEventListener === 'function' ? globalThis : undefined;
}

/** The receipts a notification presents now: each source's latest receipt
 * this browser has not acknowledged for its authority. A source native
 * retains no receipt for presents nothing, which says nothing of whether it
 * is valid. */
export function unacknowledgedRepairs(observations: Readonly<Record<string, SourceRepairObservation>> | undefined, acknowledged: RepairAcknowledgements): PresentedRepair[] {
  return Object.values(observations ?? {})
    .flatMap(({ authorityId, receipt }) => receipt ? [{ authorityId, receipt }] : [])
    .filter(repair => !acknowledged.has(repairIdentity(repair)))
    // User first, then Workspaces: a stable, owner-ordered presentation.
    .sort((a, b) => (a.receipt.target.kind === 'user' ? 0 : 1) - (b.receipt.target.kind === 'user' ? 0 : 1));
}
