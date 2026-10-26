/* Copyright (c) 2026 DeepSeek. MIT. Disclosure behavior adapted from Harness SystemPromptRow and ContextInjectionRow; see PROVENANCE.md. */
import { useEffect, useMemo, useRef, useState } from 'react';
import type { CheckpointReadCut, RuntimeClientMessageDescriptor, RuntimeClientSnapshot, RuntimeClientTranscriptEntry, TraceRecord, TraceText, TraceToolDefinition } from '../../../../protocol/app-server/v59';
import { traceAttemptKey } from '../../../../protocol/app-server/trace-identity';
import { attemptOwnerKey } from '../../../../protocol/app-server/attempt-identity';
import type { TraceFieldReader } from '../../client/trace';
import { completeTools, isRequestPredecessor, readRequestPredecessor, readSystemRequests, requestDomain, toolChanges } from '../../client/request-system';
import { readRequestField } from '../../client/trace-content';
import { useTranslation } from '../../locale/react';
import { DisclosureRow } from '../../presentation/primitives/DisclosureRow';
import { IconBrowseOutline16 } from '../../presentation/primitives/icons';
import type { RequestFeedback } from './AgentTranscript';
import { observationSourceIdentity, observationTraceReader, useObservationSource } from './ObservationSource';
import css from './SystemChanges.module.css';

type Kind = 'prompt' | 'tools';
type Snapshot = Pick<RuntimeClientSnapshot, 'conversation_id' | 'attempt'>;
const descriptorId = (descriptor: RuntimeClientMessageDescriptor) => descriptor.identity ?? (descriptor.locator.type === 'by_message_id' ? descriptor.locator.message_id : undefined);

/** Native request ownership anchors notices; loaded adjacency establishes nothing. */
export function systemChangePlacements(records: readonly TraceRecord[], entries: readonly RuntimeClientTranscriptEntry[], streamingId?: string, descriptors: readonly RuntimeClientMessageDescriptor[] = []) {
  const ids = new Set(entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : entry.item.type === 'publication_audit' ? [entry.item.audit.message_id] : []));
  for (const descriptor of descriptors) { const id = descriptorId(descriptor); if (id) ids.add(id); }
  if (streamingId) ids.add(streamingId);
  const first = new Set<string>();
  return records.flatMap(record => {
    const request = record.request, owner = traceAttemptKey(record.location);
    if (!request || !owner) return [];
    const firstRequest = !first.has(owner); first.add(owner);
    const kinds: Kind[] = [];
    if (request.system_prompt.state !== 'unchanged') kinds.push('prompt');
    if (request.tool_catalog === 'changed' || request.tool_catalog === 'previous_unavailable') kinds.push('tools');
    const publication = request.assistant_message_id != null && ids.has(request.assistant_message_id) ? request.assistant_message_id : undefined;
    const input = records.slice(0, records.indexOf(record)).find(input => input.kind === 'user' && input.message_id != null && traceAttemptKey(input.location) === owner);
    const inputId = input?.message_id;
    const user = firstRequest && request.retry_number === 0 ? entries.find(entry => entry.item.type === 'message' && entry.item.message.role === 'user' && (!entry.item.message.kind || entry.item.message.kind === 'message')
      && entry.item.message.id === inputId) : undefined;
    const deferredUser = firstRequest && request.retry_number === 0 ? descriptors.find(descriptor => descriptor.role === 'user' && (!descriptor.kind || descriptor.kind === 'message')
      && descriptorId(descriptor) === inputId && inputId != null) : undefined;
    const userId = user?.item.type === 'message' ? user.item.message.id : deferredUser && descriptorId(deferredUser);
    return kinds.flatMap(kind => {
      const beforeMessageId = kind === 'prompt' ? userId ?? publication : publication;
      // A committed publication outside this window is not an unpublished
      // request. Its notice returns with its own input/output, never at a
      // nearby retained message or another point in the same Attempt.
      return !beforeMessageId && request.accepted_message_locator != null ? [] : [{ record, kind, beforeMessageId }];
    });
  });
}

/** Read the represented Attempts from the immutable Trace domain shared by both views. */
export function useSystemChanges(snapshot: Snapshot, entries: readonly RuntimeClientTranscriptEntry[], streamingId: string | undefined, supplied?: RequestFeedback, descriptors: readonly RuntimeClientMessageDescriptor[] = []): RequestFeedback {
  const source = useObservationSource(), tx = useTranslation();
  const historyCut = source && 'historyCut' in source ? source.historyCut : undefined;
  const scope = JSON.stringify([observationSourceIdentity(source), source?.cut()?.conversation_id, historyCut]), latest = useRef({ source, scope }); latest.current = { source, scope };
  const reader = useMemo(() => source ? observationTraceReader(source) : undefined, [scope, source?.client]);
  const liveCut = source?.cut();
  const cut: CheckpointReadCut | undefined = liveCut ?? (historyCut ? { conversation_id: historyCut.read.conversation_id, revision: '0', through: historyCut.read.journal, source_cut: historyCut, domain_revisions: {}, collection_cuts: {} } : undefined);
  const owners = new Set(entries.flatMap(entry => {
    const key = entry.turn_process && traceAttemptKey(entry.turn_process);
    return key ? [key] : [];
  }));
  for (const descriptor of descriptors) { const key = descriptor.turn_process && traceAttemptKey(descriptor.turn_process); if (key) owners.add(key); }
  if (!source?.historyFrozen && snapshot.attempt) { const key = traceAttemptKey(snapshot.attempt); if (key) owners.add(key); }
  const ownerKey = JSON.stringify([...owners]), revision = JSON.stringify([cut?.domain_revisions.trace, !!cut?.source_cut, historyCut]);
  const inputIds = new Set(entries.flatMap(entry => entry.item.type === 'message' && entry.item.message.role === 'user' && (!entry.item.message.kind || entry.item.message.kind === 'message') ? [entry.item.message.id] : []));
  for (const descriptor of descriptors) if (descriptor.role === 'user' && (!descriptor.kind || descriptor.kind === 'message')) { const id = descriptorId(descriptor); if (id) inputIds.add(id); }
  const key = JSON.stringify([scope, ownerKey, [...inputIds], revision]);
  const owned = () => latest.current.scope === scope && latest.current.source?.owned() === true;
  const active = owned();
  const [retry, setRetry] = useState(0), [loaded, setLoaded] = useState<{ scope: string; owners: string; records: TraceRecord[]; cut: CheckpointReadCut; error?: boolean }>();
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  useEffect(() => {
    if (!active || !reader || !cut?.source_cut || !owners.size || !owned()) return;
    let mounted = true;
    const current = () => mounted && owned();
    void readSystemRequests(reader, cut, owners, current, inputIds).then(records => {
      if (current()) setLoaded({ scope, owners: ownerKey, records, cut });
    }).catch(() => { if (current()) setLoaded(previous => previous?.scope === scope && previous.owners === ownerKey
      ? { ...previous, error: true } : { scope, owners: ownerKey, cut, records: [], error: true }); });
    return () => { mounted = false; };
  }, [key, reader, active, retry]);
  const resident = loaded?.scope === scope && loaded.owners === ownerKey ? loaded : undefined;
  const messages = new Map(supplied?.messages), attempts = new Map(supplied?.attempts);
  if (reader && resident) for (const { record, kind, beforeMessageId } of systemChangePlacements(resident.records, entries, streamingId, descriptors)) {
    const id = `${scope}:${record.id}:${kind}`, map = beforeMessageId ? messages : attempts;
    const owner = attemptOwnerKey({ conversation_id: snapshot.conversation_id, ...record.location });
    const seat = beforeMessageId ?? owner;
    const notice = <SystemNotice key={id} scope={scope} record={record} kind={kind} records={resident.records} cut={resident.cut} reader={reader} owned={owned} open={expanded.has(id)} onToggle={() => setExpanded(previous => {
      const next = new Set(previous); if (!next.delete(id)) next.add(id); return next;
    })}/>;
    map.set(seat, <>{map.get(seat)}{notice}</>);
  }
  if (resident?.error) {
    const owner = entries.find(entry => entry.turn_process)?.turn_process ?? descriptors.find(descriptor => descriptor.turn_process)?.turn_process ?? snapshot.attempt;
    if (owner) { const seat = attemptOwnerKey({ ...owner, conversation_id: snapshot.conversation_id }); attempts.set(seat, <>{attempts.get(seat)}<p role="status">{tx('agent:system.read-error')} <button type="button" onClick={() => setRetry(value => value + 1)}>{tx('agent:message.retry-content')}</button></p></>); }
  }
  return { messages, attempts };
}

function SystemNotice({ scope, record, kind, records, cut, reader, owned, open, onToggle }: {
  scope: string; record: TraceRecord; kind: Kind; records: readonly TraceRecord[]; cut: CheckpointReadCut; reader: TraceFieldReader; owned: () => boolean; open: boolean; onToggle: () => void;
}) {
  const tx = useTranslation(), latest = useRef(owned); latest.current = owned;
  const [retry, setRetry] = useState(0), [body, setBody] = useState<{ prompt?: string; changes?: ReturnType<typeof toolChanges>; error?: boolean }>();
  const state = kind === 'prompt' ? record.request!.system_prompt.state : record.request!.tool_catalog;
  const shouldRead = open || kind === 'tools' && state === 'changed';
  const active = owned();
  useEffect(() => {
    if (!active || !shouldRead) return;
    let mounted = true;
    const current = () => mounted && latest.current();
    void (async () => {
      const domain = requestDomain(record);
      if (kind === 'prompt') {
        const prompt = await readRequestField(reader, cut, `${domain}.effective_system_prompt`, current) as TraceText;
        if (typeof prompt?.text !== 'string' || prompt.truncated) throw Error('Complete system prompt is unavailable.');
        return { prompt: prompt.text };
      }
      const previous = records.find(candidate => isRequestPredecessor(record, candidate)) ?? await readRequestPredecessor(reader, cut, record, current);
      if (!previous) throw Error('Original preceding Tool catalog is unavailable.');
      const [before, after] = await Promise.all([
        readRequestField(reader, cut, `${requestDomain(previous)}.tools`, current), readRequestField(reader, cut, `${domain}.tools`, current),
      ]) as [TraceToolDefinition[], TraceToolDefinition[]];
      if (!Array.isArray(before) || !Array.isArray(after) || !completeTools(before) || !completeTools(after)) throw Error('Complete Tool catalogs are unavailable.');
      return { changes: toolChanges(before, after) };
    })().then(value => { if (current()) setBody(value); }).catch(() => { if (current()) setBody({ error: true }); });
    return () => { mounted = false; };
  }, [scope, record.id, kind, reader, shouldRead, active, retry]);
  const changes = body?.changes;
  const title = state === 'previous_unavailable' ? tx(kind === 'prompt' ? 'trajectory:copy.previous-system-prompt-unavailable' : 'trajectory:copy.previous-tool-catalog-unavailable')
    : tx(kind === 'tools' ? 'trajectory:layout.tools-updated' : state === 'initial' ? 'trajectory:record.system-prompt' : 'trajectory:layout.system-prompt-updated');
  const summary = changes && (['added', 'removed', 'updated'] as const).filter(key => changes[key].length).map(key => tx(`agent:system.${key}-count`, { count: changes[key].length })).join(', ');
  return <div data-system-change={kind} data-request-id={record.id}>
    <DisclosureRow icon={<IconBrowseOutline16 size={14}/>} title={title} open={open} expandable expandOnRowClick onToggle={onToggle} keepContentWhenOpen
      collapsedContent={summary ? <span className={css.summary}> · {summary}</span> : undefined}>
      <div className={css.body} data-system-prompt-body={kind === 'prompt' ? '' : undefined}>
        {body?.error ? <span role="status">{tx('agent:system.read-error')} <button type="button" onClick={() => setRetry(value => value + 1)}>{tx('agent:message.retry-content')}</button></span>
          : kind === 'prompt' && body?.prompt !== undefined ? <pre>{body.prompt || tx('trajectory:record.system-prompt-missing')}</pre>
            : changes ? summary ? (['added', 'removed', 'updated'] as const).filter(key => changes[key].length).map(key => <div key={key}>{tx(`agent:system.${key}`, { names: changes[key].join(', ') })}</div>) : tx('agent:system.catalog-changed')
              : tx('common:activity.loading')}
      </div>
    </DisclosureRow>
  </div>;
}
