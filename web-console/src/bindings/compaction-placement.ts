import type { RuntimeClientProjectionHeadView, RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from '../../../protocol/app-server/v57';
import type { TranscriptCache } from '../client/transcript';
import type { TurnNode } from './turn-presentation';

/** The native T boundary seats a checkpoint after the corresponding response
 * tail. Ledger positions, Journal sequences and arrival time are different
 * domains. A checkpoint outside the reading window has no seat in that window. */
export function compactionPlacement<T extends { head: RuntimeClientProjectionHeadView }>(nodes: TurnNode[], markers: readonly T[], entries: readonly RuntimeClientTranscriptEntry[], page: RuntimeClientTranscriptPage, history?: TranscriptCache): (TurnNode | { kind: 'compaction'; marker: T })[] {
  const contains = (rows: readonly RuntimeClientTranscriptEntry[], before: string | null | undefined, anchor: bigint) => {
    const first = rows[0], last = rows.at(-1);
    return last ? anchor <= BigInt(last.cursor) && (anchor >= BigInt(first!.cursor) || before == null) : anchor === 0n && before == null;
  };
  const visible = markers.filter(({ head }) => {
    const anchor = BigInt(head.transcript_anchor);
    return history?.window
      ? contains(history.page.entries ?? [], history.page.next_cursor, anchor) || contains(page.entries ?? [], page.next_cursor, anchor)
      : contains(entries, (history?.page ?? page).next_cursor, anchor);
  }).sort((a, b) => {
    const anchor = BigInt(a.head.transcript_anchor) - BigInt(b.head.transcript_anchor);
    return anchor < 0n ? -1 : anchor > 0n ? 1 : BigInt(a.head.activation_id) < BigInt(b.head.activation_id) ? -1 : 1;
  });
  const flow: (TurnNode | { kind: 'compaction'; marker: T })[] = [...nodes];
  const inheritedThrough = (history?.page ?? page).inherited_through;
  for (const marker of visible) {
    const anchor = BigInt(marker.head.transcript_anchor);
    const index = flow.findIndex(node => node.kind === 'entry' && BigInt(node.entry.cursor) > anchor
      || node.kind === 'process' && BigInt(node.process.control_cursor) > anchor
      || node.kind === 'fork-point' && inheritedThrough != null && BigInt(inheritedThrough) >= anchor);
    flow.splice(index < 0 ? flow.length : index, 0, { kind: 'compaction', marker });
  }
  return flow;
}
