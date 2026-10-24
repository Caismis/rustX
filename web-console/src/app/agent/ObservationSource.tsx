import { createContext, useContext, useCallback, useRef, type ReactNode } from 'react';
import type { AttachmentTarget, CheckpointReadCut, ObservationReadCut, ContentRef, ContentBinding, MessageBlock, RuntimeClientTranscriptPage } from '../../../../protocol/app-server/v57';
import type { AppServerClient } from '../../client/app-server';

type Source = { client: AppServerClient; cut: () => CheckpointReadCut | undefined; owned: () => boolean; epoch?: number; historyFrozen?: boolean; content?: ContentBinding[]; messages?: MessageBlock[]; transcript?: RuntimeClientTranscriptPage; history?: RuntimeClientTranscriptPage } & (
  { target: AttachmentTarget; agentId?: string } | { sessionId: string; nodeId?: string | null; historyCut?: ObservationReadCut }
);
export function observationSourceIdentity(source?: Source) {
  const cut = source?.cut();
  return source && ('target' in source ? [source.target, source.agentId, source.agentId ? cut?.conversation_id : undefined, source.agentId ? cut?.activation_id : undefined, source.epoch] : [source.sessionId, source.nodeId, source.epoch]);
}
/** Freeze a Child read at its selected durable window, independently of the live tail. */
export function childHistoryReadCut(source: Source, history: ObservationReadCut): CheckpointReadCut {
  const live = source.cut();
  if (!('target' in source) || !source.agentId || !live?.activation_id || live.conversation_id !== history.read.conversation_id) throw Error('Original child history owner is unavailable.');
  return { conversation_id: live.conversation_id, activation_id: live.activation_id, revision: '0', through: history.read.journal,
    source_cut: history, domain_revisions: {}, collection_cuts: {} };
}
/** The advertised field stays in its native live or historical owner domain. */
export async function readObservationValue(source: Source, cut: CheckpointReadCut, domain: string, path: string[], offset: number, current: () => boolean) {
  return 'target' in source
    ? source.client.request({ method: 'conversation/checkpointValueRead', params: { target: source.target, agent_id: source.agentId ?? null, cut, domain, item_offset: 0, path, offset, max_bytes: 65_536 } }, 'checkpoint_value_chunk', undefined, current)
    : source.client.request({ method: 'session/traceValueRead', params: { session_id: source.sessionId, node_id: source.nodeId ?? null, cut, domain, item_offset: 0, path, offset, max_bytes: 65_536 } }, 'checkpoint_value_chunk', undefined, current);
}
const JournalSource = createContext<Source | undefined>(undefined);
/** Existing exact attachment/Child observation scope for selected native fields. */
export function useObservationSource() { return useContext(JournalSource); }
export function JournalSourceProvider({ source, content, messages, transcript, history, historical, children }: { historical?: boolean; source?: Source; content?: ContentBinding[]; messages?: MessageBlock[]; transcript?: RuntimeClientTranscriptPage; history?: RuntimeClientTranscriptPage; children: ReactNode }) {
  return <JournalSource value={source && { ...source, content, messages, transcript, history, historyFrozen: historical ?? source.historyFrozen }}>{children}</JournalSource>;
}

/** Selected content keeps the original live or historical authority across all chunks. */
export function useContentSource() {
  const source = useObservationSource(), latest = useRef(source); latest.current = source;
  const identity = JSON.stringify([observationSourceIdentity(source), source && 'historyCut' in source ? source.historyCut ?? source.cut() : undefined]);
  const read = useCallback(async (content_ref: ContentRef, offset: number, current: () => boolean) => {
    const captured = latest.current;
    if (!captured || !captured.owned()) throw Error('Content source is unavailable.');
    if ('target' in captured) return captured.client.reads.readContent(captured.target, captured.agentId, content_ref, offset, current);
    if (captured.historyCut) return (await captured.client.request({ method: 'session/historyContentRead', params: { session_id: captured.sessionId, node_id: captured.nodeId ?? null, cut: captured.historyCut, content_ref, offset, max_bytes: 65_536 } }, 'content_chunk', undefined, current)).chunk;
    const cut = captured.cut();
    if (!cut) throw Error('Content source is unavailable.');
    return (await captured.client.request({ method: 'session/traceContentRead', params: { session_id: captured.sessionId, node_id: captured.nodeId ?? null, cut, content_ref, offset, max_bytes: 65_536 } }, 'content_chunk', undefined, current)).chunk;
  }, [identity, source?.client]);
  return { source, read };
}

export function messageContentBinding(source: Source | undefined, id: string, index: number, publicationStream?: string) {
  return source?.content?.find(binding => {
    if (binding.content_ref.representation !== 'utf8_text') return false;
    const reference = binding.content_ref.source;
    if (reference.component !== 'payload' || reference.block_index !== index) return false;
    if (publicationStream !== undefined && (reference.type !== 'durable_transcript_audit' || reference.stream_id !== publicationStream)) return false;
    if ('message_id' in reference) return reference.message_id === id;
    if (reference.type === 'canonical_message_block' && reference.locator.type === 'by_message_id') return reference.locator.message_id === id;
    const path = binding.path;
    if (path[0] === 'messages') return source.messages?.[Number(path[1])]?.id === id;
    const page = source.history?.content?.includes(binding) ? source.history : source.transcript;
    const entryIndex = path.indexOf('entries'), entry = entryIndex >= 0 ? page?.entries?.[Number(path[entryIndex + 1])] : undefined;
    return entry?.item.type === 'message' && entry.item.message.id === id;
  });
}
