import { useEffect, useRef, useState } from 'react';
import type { AssistantContentBlock, CanonicalContentLocator, CheckpointReadCut, ContentBinding, MessageBlock, RuntimeClientMessageDescriptor, RuntimeClientTranscriptPage, UserContentBlock } from '../../../../protocol/app-server/v57';
import { messageDescriptor } from '../../../../protocol/app-server/message-descriptor';
import { canonicalLocatorKey, checkpointReadCutKey, observationReadCutKey } from '../../../../protocol/app-server/content-identity';
import { readBinding, readTraceCollection } from '../../client/trace-content';
import { childHistoryReadCut, observationSourceIdentity, useObservationSource } from './ObservationSource';

type Source = NonNullable<ReturnType<typeof useObservationSource>>;
type Block = UserContentBlock | AssistantContentBlock;
type Request = { locator: CanonicalContentLocator; offset: number; total?: string };
const requestKey = (request: Request) => JSON.stringify([canonicalLocatorKey(request.locator), request.offset, request.total]);
const object = (value: unknown): Record<string, unknown> => value != null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};

/** A frozen window and the live tail keep independent native read owners. */
function messageHistoryCut(source: Source, locator: CanonicalContentLocator) {
  if ('historyCut' in source) return source.historyCut;
  if (!source.historyFrozen) return;
  const page = source.history;
  const inline = locator.type === 'by_message_id' && page?.entries?.some(entry => entry.item.type === 'message' && entry.item.message.id === locator.message_id);
  const deferred = page?.deferred_entries?.some(item => {
    const descriptor = messageDescriptor(item.value);
    return descriptor && (canonicalLocatorKey(descriptor.locator) === canonicalLocatorKey(locator)
      || locator.type === 'by_message_id' && descriptor.identity === locator.message_id);
  });
  return inline || deferred ? page?.source_cut : undefined;
}

/** Continue the canonical message, using the same bounded pages as Trace. */
export async function readMessageBlocks(source: Source, request: Request, current: () => boolean) {
  const key = request.locator.type === 'by_message_id' ? `message:${request.locator.message_id}` : `ledger:${request.locator.position}`;
  const domain = `messages.${key}.blocks`;
  const history = messageHistoryCut(source, request.locator);
  if (source.historyFrozen && !source.history?.source_cut && !('historyCut' in source && source.historyCut)) throw Error('Original message history cut is unavailable.');
  const childHistory = history && 'target' in source && source.agentId;
  const captured = childHistory ? childHistoryReadCut(source, history) : source.cut();
  const historical = history && !childHistory;
  if (!source.owned() || !current() || !historical && !captured) throw Error('Message source is unavailable.');
  const page = async (cut: CheckpointReadCut | undefined, offset: number, owns: () => boolean) => {
    const result = historical
      ? await source.client.request({ method: 'session/historyPage', params: { session_id: 'target' in source ? source.target.session_id : source.sessionId, node_id: 'nodeId' in source ? source.nodeId : null, cut: history, domain, offset, limit: 32 } }, 'checkpoint_page', undefined, owns)
      : 'target' in source
        ? await source.client.request({ method: 'conversation/checkpointPage', params: { target: source.target, agent_id: source.agentId ?? null, cut: cut!, domain, offset, limit: 32 } }, 'checkpoint_page', undefined, owns)
        : await source.client.request({ method: 'session/tracePage', params: { session_id: source.sessionId, node_id: source.nodeId ?? null, cut: cut!, domain, offset, limit: 32 } }, 'checkpoint_page', undefined, owns);
    if (historical ? (!result.page.read_cut.source_cut || observationReadCutKey(result.page.read_cut.source_cut) !== observationReadCutKey(history)) : checkpointReadCutKey(result.page.read_cut) !== checkpointReadCutKey(captured!)) throw Error('Original message cut changed.');
    return result.page;
  };
  const first = await page(captured, request.offset, current), cut = first.read_cut;
  const reader = {
    page: (at: CheckpointReadCut, _domain: string, offset: number, owns: () => boolean) => page(at, offset, owns),
    content: async (_cut: CheckpointReadCut, reference: ContentBinding['content_ref'], offset: number, owns: () => boolean) => {
      if ('target' in source && !historical) return source.client.reads.readContent(source.target, source.agentId, reference, offset, owns);
      const result = historical
        ? await source.client.request({ method: 'session/historyContentRead', params: { session_id: 'target' in source ? source.target.session_id : source.sessionId, node_id: 'nodeId' in source ? source.nodeId : null, cut: history, content_ref: reference, offset, max_bytes: 65536 } }, 'content_chunk', undefined, owns)
        : await source.client.request({ method: 'session/traceContentRead', params: { session_id: 'sessionId' in source ? source.sessionId : source.target.session_id, node_id: 'nodeId' in source ? source.nodeId : null, cut, content_ref: reference, offset, max_bytes: 65536 } }, 'content_chunk', undefined, owns);
      return result.chunk;
    },
  };
  const rows = await readTraceCollection(reader, cut, { domain, total: request.total ?? first.total }, current, first, request.offset);
  const blocks: Block[] = [];
  for (const row of rows) {
    for (const binding of row.content) {
      const ref = binding.content_ref, origin = ref.source;
      if (ref.conversation_id !== cut.conversation_id || origin.type !== 'canonical_message_block' || origin.block_index !== Number(row.offset)
        || origin.locator.type === request.locator.type && canonicalLocatorKey(origin.locator) !== canonicalLocatorKey(request.locator)) throw Error('Original message block owner changed.');
    }
    let metadata = object(row.value);
    const header = row.content.find(binding => binding.content_ref.source.component === 'block_metadata');
    if (row.value == null && header) metadata = object(await readBinding(reader, cut, header, current));
    const payload = row.content.find(binding => binding.content_ref.source.component === 'payload' && binding.content_ref.representation !== 'binary_bytes');
    if (payload) {
      const value = await readBinding(reader, cut, payload, current);
      if (['text','reasoning','refusal'].includes(row.identity)) metadata = { ...metadata, text: value };
      else if (row.identity === 'tool_call') metadata = { ...metadata, arguments: typeof value === 'string' ? value : JSON.stringify(value) };
      else metadata = { ...metadata, ...object(value) };
    }
    const block = { ...metadata, type: row.identity };
    if (['text','reasoning','refusal'].includes(row.identity) && typeof metadata.text !== 'string'
      || row.identity === 'uploaded_file' && (typeof metadata.name !== 'string' || typeof metadata.batch_id !== 'string')
      || ['image','file'].includes(row.identity) && typeof metadata.artifact_id !== 'string'
      || row.identity === 'tool_call' && (typeof metadata.name !== 'string' || typeof metadata.arguments !== 'string')
      || !['text','reasoning','refusal','uploaded_file','image','file','tool_call'].includes(row.identity)) throw Error('Original message block is unavailable.');
    blocks.push(block as Block);
  }
  return blocks;
}

/** Read immutable continuations behind the normal message seats and segmentation. */
export function useMessageBlocks(requests: readonly Request[]) {
  const source = useObservationSource();
  const identity = JSON.stringify([observationSourceIdentity(source), source?.historyFrozen && source.history?.source_cut, source && 'historyCut' in source ? source.historyCut : undefined]);
  const plan = JSON.stringify(requests), latest = useRef({ identity, plan, source }); latest.current = { identity, plan, source };
  const [retry, setRetry] = useState(0);
  const [state, setState] = useState<{ identity: string; blocks: Record<string, Block[]>; errors: Record<string, { error: string; revision?: string }> }>({ identity, blocks: {}, errors: {} });
  const loaded = state.identity === identity ? state : { identity, blocks: {}, errors: {} };
  const revision = source?.cut()?.revision;
  const requestRevision = (request: Request) => source && messageHistoryCut(source, request.locator) ? undefined : revision;
  const activeRequests = new Map(requests.map(request => [requestKey(request), request]));
  const errors = Object.entries(loaded.errors).flatMap(([key,error]) => {
    const request = activeRequests.get(key);
    return request ? [{ ...error, currentRevision: requestRevision(request) }] : [];
  });
  const retryVersion = errors.some(error => error.currentRevision !== undefined) ? revision : undefined, active = !!source?.owned();
  useEffect(() => {
    if (!source || !active) return;
    let mounted = true;
    const current = () => mounted && latest.current.identity === identity && latest.current.plan === plan && !!latest.current.source?.owned();
    void (async () => {
      for (const request of requests) {
        const key = requestKey(request), atRevision = requestRevision(request);
        if (loaded.blocks[key] || loaded.errors[key] && loaded.errors[key].revision === atRevision) continue;
        try {
          const blocks = await readMessageBlocks(source, request, current);
          if (current()) setState(previous => {
            const errors = { ...(previous.identity === identity ? previous.errors : {}) }; delete errors[key];
            return { identity, blocks: { ...(previous.identity === identity ? previous.blocks : {}), [key]: blocks }, errors };
          });
        } catch (error) {
          if (current()) setState(previous => ({ identity, blocks: previous.identity === identity ? previous.blocks : {}, errors: { ...(previous.identity === identity ? previous.errors : {}), [key]: { error: String(error), revision: atRevision } } }));
        }
        if (!current()) return;
      }
    })();
    return () => { mounted = false; };
  }, [identity, plan, source?.client, retry, retryVersion, active]);
  return { blocks: (request: Request) => loaded.blocks[requestKey(request)], errors: errors.filter(error => error.revision === error.currentRevision).map(error => error.error), retry: () => { setState(previous => ({ ...previous, errors: {} })); setRetry(value => value + 1); } };
}
export function messageContinuation(message: MessageBlock, descriptors: readonly RuntimeClientMessageDescriptor[], page: RuntimeClientTranscriptPage): Request | undefined {
  if (message.role === 'tool') return;
  const descriptor = descriptors.find(descriptor => descriptor.identity === message.id);
  const count = message.content.length;
  if (descriptor ? BigInt(descriptor.block_count) <= BigInt(count) : count < 32 || !page.collections?.some(frontier => frontier.domain.endsWith('.blocks'))) return;
  return { locator: { type: 'by_message_id', message_id: message.id }, offset: count, total: descriptor?.block_count };
}
