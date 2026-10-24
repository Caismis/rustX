/**
 * Pure client rules for native automatic `rustx.toml` repair observations
 * (Issue #464), shared by every client (Issue #487).
 *
 * The App Server owns repair: classification, validation, backup,
 * publication and the receipt itself. A client only keeps the latest
 * observation of each configuration source. This module is the one place
 * that orders two observations of a source, decides when an observation is
 * ambiguous and its source must be read again, and bounds how many sources a
 * client keeps. Notices, announcement and acknowledgment stay with each
 * consumer. Pure and browser-safe: generated types only.
 */
import type { ConfigRepairReceipt, SourceTarget } from './v57.ts';

/** Workspaces whose repair observations a client keeps: as many as the
 * native repair ledger retains (`MAX_LEDGER_WORKSPACES`). The User source has
 * its own slot beside them. */
export const SOURCE_REPAIR_WORKSPACES = 64;

/** The stable client key of one configuration source. */
export function sourceRepairKey(target: SourceTarget): string {
  return target.kind === 'user' ? 'user' : `workspace:${target.directory}`;
}

/** Whether native ledger sequence `a` is older than `b`; both are exact
 * decimal sequences of one App Server authority. */
export function olderRepairSequence(a: string, b: string): boolean {
  return BigInt(a) < BigInt(b);
}

/** The later of two native ledger sequences of one authority. */
export function laterRepairSequence(a: string, b: string): string {
  return olderRepairSequence(a, b) ? b : a;
}

/**
 * One native observation of a source's latest repair receipt, or of none, at
 * the native repair-ledger `sequence` it is current at: the `sequence` of a
 * `configuration/sourceRepairChanged`, or the `repair_sequence` of a
 * `configuration/sourcesRead`. Within one App Server authority the higher
 * sequence is the newer observation of a source, whichever carried it.
 * Receipt ids identify content, never time, and never order observations.
 */
export interface SourceRepairLevel {
  readonly target: SourceTarget;
  readonly receipt?: ConfigRepairReceipt;
  readonly sequence: string;
}

/** What a client does with one observation: keep it, drop it, or drop it and
 * read its source again because what it holds may be wrong or missing. */
export type SourceRepairDecision = 'apply' | 'ignore' | 'recover';

/**
 * Orders one native observation against what this connection holds, or may
 * have held, of its source.
 *
 * A held source orders it by sequence alone: a higher sequence replaces it,
 * even with the same receipt or with no receipt on either side, so a
 * withdrawal retires the receipt and an unchanged one advances. Two
 * observations at one sequence agree; one that disagrees is inconsistent,
 * never newer, so the held one stays and a notice's source is read again. An
 * absent source may have been forgotten through `forgotten` — for a read,
 * its own bound; for a notice (`forgotten` undefined), the connection's
 * `forgottenThrough` frontier — and an observation older than that is never
 * applied; an ambiguous notice is recovered by reading its source again. The
 * User is never forgotten.
 */
export function sourceRepairDecision(
  known: SourceRepairLevel | undefined,
  observation: SourceRepairLevel,
  forgotten: string | undefined,
  forgottenThrough: string,
): SourceRepairDecision {
  if (known !== undefined) {
    if (olderRepairSequence(observation.sequence, known.sequence)) return 'ignore';
    if (!olderRepairSequence(known.sequence, observation.sequence)) {
      return observation.receipt?.id !== known.receipt?.id && forgotten === undefined ? 'recover' : 'ignore';
    }
    return 'apply';
  }
  if (observation.target.kind !== 'user' && olderRepairSequence(observation.sequence, forgotten ?? forgottenThrough)) {
    return forgotten === undefined ? 'recover' : 'ignore';
  }
  return 'apply';
}

/**
 * The one client retention policy for per-source repair state, applied by
 * each owner to its own collection: `value` becomes `key`'s entry and its
 * most recently observed one; beyond `SOURCE_REPAIR_WORKSPACES` Workspaces
 * the least recently observed Workspace is forgotten, whether or not the User
 * has an entry, and the User never is. `sources` (least recently observed
 * first) is never modified. Returns the retained entries in the same order
 * and the forgotten entry, whose owner retires whatever it held for that
 * source.
 */
export function retainSourceRepair<V>(
  sources: Iterable<readonly [string, V]>,
  key: string,
  value: V,
): { retained: [string, V][]; forgotten?: V } {
  const retained: [string, V][] = [];
  for (const [source, entry] of sources) if (source !== key) retained.push([source, entry]);
  retained.push([key, value]);
  const workspaces = retained.filter(([source]) => source !== 'user').length;
  if (workspaces <= SOURCE_REPAIR_WORKSPACES) return { retained };
  const at = retained.findIndex(([source]) => source !== 'user');
  const [evicted] = retained.splice(at, 1);
  return { retained, forgotten: evicted?.[1] };
}
