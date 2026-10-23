import { canonicalLocatorKey } from '../../../protocol/app-server/content-identity';
import type { CanonicalContentLocator, RuntimeClientDeliveryHead, RuntimeClientDeliveryItem, RuntimeClientMessageDescriptor, RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage, TurnProcessView } from '../../../protocol/app-server/v54';
import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity';
import { messageDescriptor } from '../../../protocol/app-server/message-descriptor';
import type { ForegroundToolExecution, SessionFileReference, ToolExecutionResult } from '../../../protocol/app-server/v54';
import type { PresentRowView } from '../presentation/agent/PresentRow';

/** Raw arguments can be partial while a call streams; then they show verbatim. */
function declaredPaths(raw: string): string {
  let args: unknown;
  try { args = JSON.parse(raw); } catch { return raw; }
  if (typeof args !== 'object' || args === null || !('files' in args) || !Array.isArray(args.files)) return raw;
  return args.files.flatMap((file: unknown) =>
    typeof file === 'object' && file !== null && 'path' in file && typeof file.path === 'string' ? [file.path] : []).join(', ');
}

/** Native canonical declaration status is independent of execution settlement. */
export function presentRow(tool: ForegroundToolExecution): PresentRowView {
  const result = tool.committed_result ?? (tool.state.type === 'settled' ? tool.state.result : undefined);
  const status = result?.status;
  const phase = !status ? (tool.state.type === 'running' ? 'running' : 'preparing')
    : status.type === 'success' ? 'ok' : status.type === 'cancelled' ? 'stopped' : 'error';
  const reason = status && 'error' in status ? status.error : status && 'reason' in status ? status.reason : status && 'detail' in status ? status.detail : undefined;
  const output = (result?.content ?? []).flatMap(block => block.type === 'text' ? [block.text] : []).join('\n');
  return { id: tool.call_id??undefined, phase, paths: declaredPaths(tool.state.arguments), details: [reason, output].filter(Boolean).join('\n') };
}

/** One committed delivery and its stable native address. */
export interface PresentedDelivery { readonly key: string; readonly locator: CanonicalContentLocator; readonly index: number; readonly file: SessionFileReference }

/**
 * The typed deliveries of one committed canonical Tool-result message, in
 * canonical order. Only a successful result delivers; arguments, content JSON
 * and prose are never read.
 */
export function presentedDeliveries(locator: CanonicalContentLocator, result: ToolExecutionResult): PresentedDelivery[] {
  if (result.status.type !== 'success') return [];
  return (result.deliveries ?? []).map((file, index) => ({ key: `${canonicalLocatorKey(locator)}:${index}`, locator, index, file }));
}

/** One original committed result; bounded heads retain their native read domain. */
export type DeliveryBatch = { position: string } & ({ deliveries: PresentedDelivery[]; head?: never } | { head: RuntimeClientDeliveryHead; deliveries?: never });

/** Canonical ledger coordinates never stand in for transcript positions. */
export function descriptorTranscriptPosition(descriptor: RuntimeClientMessageDescriptor, entries: readonly RuntimeClientTranscriptEntry[], page: RuntimeClientTranscriptPage) {
  const id = descriptor.identity ?? (descriptor.locator.type === 'by_message_id' ? descriptor.locator.message_id : undefined);
  const entry = id == null ? undefined : entries.find(entry => entry.item.type === 'message' && entry.item.message.id === id);
  if (entry) return entry.cursor;
  return page.deferred_entries?.find(row => {
    const candidate = messageDescriptor(row.value);
    return candidate && canonicalLocatorKey(candidate.locator) === canonicalLocatorKey(descriptor.locator);
  })?.offset;
}

/** Exact native membership, never adjacent prose, assigns declarations to a Turn. */
export function turnDeliveries(entries: readonly RuntimeClientTranscriptEntry[], descriptors: readonly RuntimeClientMessageDescriptor[], page: RuntimeClientTranscriptPage) {
  const turns = new Map<string, { owner: TurnProcessView; batches: DeliveryBatch[] }>();
  const add = (owner: TurnProcessView, batch: DeliveryBatch) => {
    const key = attemptOwnerKey(owner), turn = turns.get(key) ?? { owner, batches: [] };
    turn.owner = owner;
    const index = turn.batches.findIndex(value => value.position === batch.position);
    if (index < 0) turn.batches.push(batch); else turn.batches[index] = batch;
    turns.set(key, turn);
  };
  for (const entry of entries) {
    if (!entry.turn_process || entry.item.type !== 'message' || entry.item.message.role !== 'tool') continue;
    const deliveries = presentedDeliveries({ type: 'by_message_id', message_id: entry.item.message.id }, entry.item.message.result);
    if (deliveries.length) add(entry.turn_process, { position: entry.cursor, deliveries });
  }
  for (const descriptor of descriptors) {
    if (!descriptor.deliveries) continue;
    const position = descriptorTranscriptPosition(descriptor, entries, page);
    const entry = entries.find(entry => entry.cursor === position);
    const owner = descriptor.turn_process ?? entry?.turn_process;
    if (owner && position) add(owner, { position, head: descriptor.deliveries });
  }
  for (const turn of turns.values()) turn.batches.sort((a, b) => BigInt(a.position) < BigInt(b.position) ? -1 : 1);
  return turns;
}

/** Harness keeps first-seen path order and the latest declaration's exact address. */
export function latestTurnDeliveries(batches: readonly { deliveries?: readonly PresentedDelivery[]; head?: { items: readonly RuntimeClientDeliveryItem[] } }[]): PresentedDelivery[] {
  const files = new Map<string, PresentedDelivery>();
  for (const batch of batches) {
    const deliveries = batch.head ? batch.head.items.flatMap(item => item.file ? [{ key: `${canonicalLocatorKey(item.locator)}:${item.index}`, locator: item.locator, index: item.index, file: item.file }] : []) : batch.deliveries ?? [];
    for (const delivery of deliveries) {
      const { conversation_id, device, inode } = delivery.file.scope;
      files.set(JSON.stringify([conversation_id, device, inode, delivery.file.path]), delivery);
    }
  }
  return [...files.values()];
}
