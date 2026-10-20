import type { AttachmentTarget, CompletedResponseView } from '../../../../protocol/app-server/v51';
import { attemptOwnerKey } from '../../../../protocol/app-server/attempt-identity';
import { canonicalLocatorKey } from '../../../../protocol/app-server/content-identity';
import { sameTarget, type SessionView } from '../../client/app-server';
import { transcriptPresentation } from '../../client/transcript';

/** A disposable reading prefix, never an attachment or execution authority. */
export interface ResponseRetry {
  view: SessionView;
  replay?: { target: AttachmentTarget; messageId: string };
}

export function responseRetry(view: SessionView | undefined, response: CompletedResponseView): ResponseRetry | undefined {
  if (!view?.snapshot) return;
  const entries = transcriptPresentation(view.history, view.snapshot.transcript);
  const locator = response.retry_locator;
  const userId = response.retry_message_id ?? (locator?.type === 'by_message_id' ? locator.message_id
    : locator && view.snapshot.message_descriptors?.find(descriptor => canonicalLocatorKey(descriptor.locator) === canonicalLocatorKey(locator))?.identity);
  const user = entries.find(entry => entry.item.type === 'message' && entry.item.message.id === userId);
  const owner = attemptOwnerKey(response.origin);
  const process = entries.find(entry => entry.turn_process && attemptOwnerKey(entry.turn_process) === owner)?.turn_process;
  const closing = entries.find(entry => entry.completed_response && attemptOwnerKey(entry.completed_response.origin) === owner);
  // A paged-out input does not authorize borrowing a neighbouring User row.
  const through = user?.cursor;
  const before = process?.control_cursor ?? closing?.cursor;
  const prefix = entries.filter(entry => through != null ? BigInt(entry.cursor) <= BigInt(through)
    : before != null && BigInt(entry.cursor) < BigInt(before)).map(entry => entry === user
      ? { ...entry, turn_process: undefined, completed_response: undefined } : entry);
  const ids = new Set(prefix.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []));
  const inputIndex = view.snapshot.messages.findIndex(message => message.id === userId);
  const transcript = { ...view.snapshot.transcript, entries: prefix };
  return { view: { ...view, snapshot: { ...view.snapshot, messages: view.snapshot.messages.filter((message, index) => index <= inputIndex || ids.has(message.id)),
    attempt: null, transcript, message_descriptors: view.snapshot.message_descriptors?.filter(descriptor => descriptor.identity != null
      && (ids.has(descriptor.identity) || descriptor.identity === userId)), publication_settlements: [], pending_interactions: [] },
    publicationHeads: [], history: { epoch: view.history?.epoch ?? 0,
      page: { ...(view.history?.page ?? view.snapshot.transcript), entries: prefix } } } };
}

/** Hand off only to the native accepted replay input in its exact Conversation. */
export function responseRetryReady(retry: ResponseRetry, view: SessionView | undefined): boolean {
  const replay = retry.replay;
  return !!replay && sameTarget(view?.target, replay.target) && !!view?.snapshot
    && (view.snapshot.messages.some(message => message.id === replay.messageId)
      || view.snapshot.message_descriptors?.some(descriptor => descriptor.identity === replay.messageId)
      || transcriptPresentation(view.history, view.snapshot.transcript).some(entry => entry.item.type === 'message' && entry.item.message.id === replay.messageId));
}
