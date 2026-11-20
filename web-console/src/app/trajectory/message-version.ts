import { canonicalLocatorKey } from '../../../../protocol/app-server/content-identity';
import type { SessionMessageVersionFamily, TraceRecord } from '../../../../protocol/app-server/v80';

const messageBlocks = /^messages\.ledger:([1-9][0-9]*)\.blocks$/;

/** A version shares Ledger positions with inherited history. Each node
 * allocates its own message IDs. */
export function traceUserVersion(
  record: TraceRecord,
  versions: ReadonlyMap<string, SessionMessageVersionFamily>,
) {
  if (record.kind !== 'user' || versions.size === 0) return;
  let position: number | undefined;
  for (const { domain } of record.collections) {
    const match = messageBlocks.exec(domain);
    if (!match) continue;
    // Multiple adopted messages have individual controls in the inspector.
    if (position !== undefined) return;
    position = Number(match[1]);
    if (!Number.isSafeInteger(position)) return;
  }
  if (position !== undefined) return versions.get(canonicalLocatorKey({ type: 'by_ledger_position', position }));
}
