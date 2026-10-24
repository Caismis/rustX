import { useEffect, useRef, useState } from 'react';
import type { CanonicalContentLocator, ContentBinding, ForegroundToolExecution } from '../../../../protocol/app-server/v57';
import { ContentCache } from '../../../../protocol/app-server/content-cache';
import { canonicalLocatorKey, checkpointReadCutKey, observationReadCutKey } from '../../../../protocol/app-server/content-identity';
import { readContentText } from '../../client/content-text';
import { childHistoryReadCut, observationSourceIdentity, useContentSource, useObservationSource } from './ObservationSource';

type Source = NonNullable<ReturnType<typeof useObservationSource>>;
/** One native occurrence, including calls excluded by an inline header's byte budget. */
export async function readToolOccurrence(source: Source, locator: CanonicalContentLocator, blockIndex: number, current: () => boolean, frozen = false) {
  const key = locator.type === 'by_message_id' ? `message:${locator.message_id}` : `ledger:${locator.position}`;
  const domain = `messages.${key}.tools`;
  const history = 'historyCut' in source ? source.historyCut : frozen ? source.history?.source_cut : undefined;
  if (!current() || !source.owned()) throw Error('Tool source is unavailable.');
  if (frozen && !history) throw Error('Original Tool history cut is unavailable.');
  const childHistory = history && 'target' in source && source.agentId;
  const cut = childHistory ? childHistoryReadCut(source, history) : source.cut();
  const result = history && !('target' in source && source.agentId)
    ? await source.client.request({ method: 'session/historyPage', params: { session_id: 'target' in source ? source.target.session_id : source.sessionId, node_id: 'nodeId' in source ? source.nodeId : null, cut: history, domain, offset: blockIndex, limit: 1 } }, 'checkpoint_page', undefined, current)
    : 'target' in source && cut
      ? await source.client.request({ method: 'conversation/checkpointPage', params: { target: source.target, agent_id: source.agentId ?? null, cut, domain, offset: blockIndex, limit: 1 } }, 'checkpoint_page', undefined, current)
      : !('target' in source) && cut
        ? await source.client.request({ method: 'session/tracePage', params: { session_id: source.sessionId, node_id: source.nodeId ?? null, cut, domain, offset: blockIndex, limit: 1 } }, 'checkpoint_page', undefined, current)
        : undefined;
  if (!current()) throw Error('Tool view changed.');
  const page = result?.page, item = page?.items[0];
  if (!page || page.domain !== domain || page.items.length !== 1 || item?.offset !== String(blockIndex)
    || (history && !childHistory ? (!page.read_cut.source_cut || observationReadCutKey(page.read_cut.source_cut) !== observationReadCutKey(history)) : checkpointReadCutKey(page.read_cut) !== checkpointReadCutKey(cut!))) throw Error('Original Tool occurrence is unavailable.');
  const tool = item.value as ForegroundToolExecution | undefined;
  if (!tool || tool.block_index !== blockIndex || tool.occurrence?.block_index !== blockIndex || typeof tool.name !== 'string'
    || !['assembled', 'running', 'settled'].includes(tool.state?.type) || typeof tool.state.arguments !== 'string') throw Error('Original Tool descriptor is unavailable.');
  if (locator.type === 'by_ledger_position' && canonicalLocatorKey(tool.occurrence.locator) !== canonicalLocatorKey(locator)
    || locator.type === 'by_message_id' && tool.message_id != null && tool.message_id !== locator.message_id) throw Error('Original Tool occurrence changed.');
  return { tool, content: item.content };
}

/** Input bytes are immutable; reading them never replaces execution settlement. */
export function useToolInput(tool: ForegroundToolExecution) {
  const { source, read } = useContentSource();
  const identity = JSON.stringify([observationSourceIdentity(source), canonicalLocatorKey(tool.occurrence.locator), tool.block_index]);
  const latest = useRef({ identity, source }); latest.current = { identity, source };
  const [requested, request] = useState(false), [retry, setRetry] = useState(0);
  const [state, setState] = useState<{ identity: string; arguments: string; error?: string }>();
  const loaded = state?.identity === identity ? state : undefined;
  const deferred = tool.assembly_complete && tool.state.arguments === '', active = !!source?.owned();
  useEffect(() => {
    if (!requested || !deferred || !source || !active || loaded && !loaded.error) return;
    let mounted = true;
    const current = () => mounted && latest.current.identity === identity && !!latest.current.source?.owned();
    const cache = new ContentCache((ref, offset, _max, owned) => read(ref, offset, owned));
    void (async () => {
      const original = await readToolOccurrence(source, tool.occurrence.locator, tool.block_index, current);
      let args = original.tool.state.arguments;
      if (args === '') {
        const binding = original.content.find((binding: ContentBinding) => binding.path.join('.') === 'state.arguments');
        if (!binding) throw Error('Original Tool arguments are unavailable.');
        args = await readContentText(cache, binding.content_ref, current);
      }
      if (current()) setState({ identity, arguments: args });
    })().catch(error => { if (current()) setState({ identity, arguments: '', error: String(error) }); });
    return () => { mounted = false; cache.clear(); };
  }, [identity, requested, deferred, retry, read, source?.client, active]);
  return { tool: loaded && !loaded.error ? { ...tool, state: { ...tool.state, arguments: loaded.arguments } } : tool,
    pending: requested && deferred && active && !loaded, error: loaded?.error ?? (requested && deferred && !active ? 'Tool source is unavailable.' : undefined),
    expand: () => request(true), retry: () => { setState(undefined); setRetry(value => value + 1); } };
}
