import type { AttachmentTarget, CompletedResponseView, RuntimeClientMessageDescriptor, RuntimeClientTranscriptPage } from '../../../../protocol/app-server/v57';
import { attemptOwnerKey } from '../../../../protocol/app-server/attempt-identity';
import { canonicalLocatorKey } from '../../../../protocol/app-server/content-identity';
import { sameTarget } from '../../../../protocol/app-server/attachment-identity';
import type { SessionView } from '../../client/contracts';
import { messageDescriptor } from '../../../../protocol/app-server/message-descriptor';
import { transcriptPresentation } from '../../client/transcript';

/** A disposable reading prefix, never an attachment or execution authority. */
export interface ResponseRetry {
  view: SessionView;
  replay?: { target: AttachmentTarget; messageId: string };
}

export function responseRetry(view: SessionView | undefined, response: CompletedResponseView): ResponseRetry | undefined {
  if (!view?.snapshot) return;
  const entries = transcriptPresentation(view.history, view.snapshot.transcript);
  const pages = [view.snapshot.transcript, ...(view.history ? [view.history.page] : [])];
  const deferred = pages.flatMap(page => page.deferred_entries ?? []);
  const locator = response.retry_locator;
  const userId = response.retry_message_id ?? (locator?.type === 'by_message_id' ? locator.message_id
    : locator && [...view.snapshot.message_descriptors ?? [], ...deferred.flatMap(entry => { const descriptor = messageDescriptor(entry.value); return descriptor ? [descriptor] : []; })].find(descriptor => canonicalLocatorKey(descriptor.locator) === canonicalLocatorKey(locator))?.identity);
  const user = entries.find(entry => entry.item.type === 'message' && entry.item.message.id === userId);
  const isInput = (descriptor: RuntimeClientMessageDescriptor | undefined) => !!descriptor && (userId != null && descriptor.identity === userId
    || !!locator && canonicalLocatorKey(descriptor.locator) === canonicalLocatorKey(locator));
  const deferredUser = deferred.find(entry => isInput(messageDescriptor(entry.value)));
  const owner = attemptOwnerKey(response.origin);
  const facts = [...entries, ...deferred.map(entry => ({ cursor: entry.offset, ...messageDescriptor(entry.value) }))];
  const process = facts.find(entry => entry.turn_process && attemptOwnerKey(entry.turn_process) === owner)?.turn_process;
  const closing = facts.find(entry => entry.completed_response && attemptOwnerKey(entry.completed_response.origin) === owner);
  // A paged-out input does not authorize borrowing a neighbouring User row.
  const through = user?.cursor ?? deferredUser?.offset;
  const before = process?.control_cursor ?? closing?.cursor;
  const retained = (cursor: string) => through != null ? BigInt(cursor) <= BigInt(through) : before != null && BigInt(cursor) < BigInt(before);
  const prefix = entries.filter(entry => retained(entry.cursor)).map(entry => entry === user
      ? { ...entry, turn_process: undefined, completed_response: undefined } : entry);
  const ids = new Set(prefix.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []));
  const positions = new Map(prefix.map((entry, index) => [entry.cursor, index]));
  const inputIndex = view.snapshot.messages.findIndex(message => message.id === userId);
  const truncate = (page: RuntimeClientTranscriptPage): RuntimeClientTranscriptPage => {
    const deferred_entries = (page.deferred_entries ?? []).filter(entry => retained(entry.offset)).map(entry =>
      isInput(messageDescriptor(entry.value)) ? { ...entry, value: { ...messageDescriptor(entry.value)!, turn_process: undefined, completed_response: undefined } } : entry);
    const deferredPositions = new Map(deferred_entries.map((entry, index) => [entry.offset, index]));
    const content = (page.content ?? []).flatMap(binding => {
      const path = [...binding.path], kind = path[0];
      if (kind !== 'entries' && kind !== 'deferred_entries') return [];
      const index = Number(path[1]);
      const cursor = kind === 'entries' ? page.entries?.[index]?.cursor : page.deferred_entries?.[index]?.offset;
      const next = cursor == null ? undefined : (kind === 'entries' ? positions : deferredPositions).get(cursor);
      if (next == null) return [];
      path[1] = String(next);
      return [{ ...binding, path }];
    });
    return { ...page, entries: prefix, deferred_entries, content };
  };
  const transcript = truncate(view.snapshot.transcript);
  const history = truncate(view.history?.page ?? view.snapshot.transcript);
  const locators = new Set([...transcript.deferred_entries ?? [], ...history.deferred_entries ?? []].flatMap(entry => {
    const descriptor = messageDescriptor(entry.value); return descriptor ? [canonicalLocatorKey(descriptor.locator)] : [];
  }));
  return { view: { ...view, snapshot: { ...view.snapshot, messages: view.snapshot.messages.filter((message, index) => index <= inputIndex || ids.has(message.id)),
    attempt: null, transcript, message_descriptors: view.snapshot.message_descriptors?.filter(descriptor => locators.has(canonicalLocatorKey(descriptor.locator)) || isInput(descriptor)
      || descriptor.identity != null && ids.has(descriptor.identity)).map(descriptor => isInput(descriptor)
        ? { ...descriptor, turn_process: undefined, completed_response: undefined } : descriptor), publication_settlements: [], pending_interactions: [] },
    publicationHeads: [], history: { epoch: view.history?.epoch ?? 0,
      page: history } } };
}

/** Hand off only to the native accepted replay input in its exact Conversation. */
export function responseRetryReady(retry: ResponseRetry, view: SessionView | undefined): boolean {
  const replay = retry.replay;
  return !!replay && sameTarget(view?.target, replay.target) && !!view?.snapshot
    && (view.snapshot.messages.some(message => message.id === replay.messageId)
      || view.snapshot.message_descriptors?.some(descriptor => descriptor.identity === replay.messageId)
      || transcriptPresentation(view.history, view.snapshot.transcript).some(entry => entry.item.type === 'message' && entry.item.message.id === replay.messageId));
}
