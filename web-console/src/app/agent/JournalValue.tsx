import { attemptIdentitySourceKey } from '../../../../protocol/app-server/attempt-identity';
import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import type { AttemptIdentitySource, AttachmentTarget, CheckpointReadCut, ContentRef, JournalFieldRef, ObservationReadCut, RuntimeClientSnapshot } from '../../../../protocol/app-server/v52';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { DeferredContent } from './DeferredContent';

type Source = { client: AppServerClient; cut: () => CheckpointReadCut | undefined; owned: () => boolean } & (
  { target: AttachmentTarget; agentId?: string } | { sessionId: string; nodeId?: string | null; historyCut?: ObservationReadCut }
);
export function observationSourceIdentity(source?: Source) {
  return source && ('target' in source ? [source.target, source.agentId] : [source.sessionId, source.nodeId]);
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
export function JournalSourceProvider({ source, children }: { source?: Source; children: ReactNode }) {
  return <JournalSource value={source}>{children}</JournalSource>;
}

/** A canonical Tool metadata result retains its original structural bytes. */
export function ToolMetadataValue({ reference }: { reference?: ContentRef | null }) {
  const source = useContext(JournalSource);
  if (!reference || !source || !('target' in source)) return null;
  return <DeferredContent client={source.client} target={source.target} agentId={source.agentId}
    bindings={[{ path: ['result'], source_path: ['result'], content_ref: reference, deferred: true }]} owned={source.owned}/>;
}

/** Original final-message metadata is read only from a proven native completion. */
export function TurnFinalMessageValue({ reference }: { reference?: ContentRef | null }) {
  const source=useContext(JournalSource),cut=source?.cut(),historyCut=source&&'sessionId' in source?source.historyCut:undefined;
  if(!reference||!source)return null;
  return <DeferredContent client={source.client} target={'target' in source?source.target:undefined}
    agentId={'target' in source?source.agentId:undefined}
    bindings={[{path:['final_message_id'],content_ref:reference,deferred:true}]} owned={source.owned}
    read={'sessionId' in source&&historyCut?async(ref,offset,current)=>(await source.client.request({method:'session/historyContentRead',params:{session_id:source.sessionId,node_id:source.nodeId??null,cut:historyCut,content_ref:ref,offset,max_bytes:65_536}},'content_chunk',undefined,current)).chunk
      :'sessionId' in source&&cut?async(ref,offset,current)=>(await source.client.request({method:'session/traceContentRead',params:{session_id:source.sessionId,node_id:source.nodeId??null,cut,content_ref:ref,offset,max_bytes:65_536}},'content_chunk',undefined,current)).chunk:undefined}/>;
}

/** Execution settlement keeps its native diagnostic source independently of display bytes. */
export function AttemptJournalValues({ attempt }: { attempt?: RuntimeClientSnapshot['attempt'] }) {
  if (attempt?.phase.type !== 'settled') return null;
  const outcome = attempt.phase.outcome;
  const field = outcome.type === 'failed' ? outcome.error_source
    : outcome.type === 'completed' ? outcome.finish_reason_source : undefined;
  return <JournalValue field={field ?? undefined}/>;
}

/** One original native J field chunk; never parses or rewrites opaque JSON. */
export function JournalValue({ field }: { field?: JournalFieldRef }) {
  return field && <NativeFieldValue domain={`journal.${field.sequence}`} path={field.path} encodedBytes={field.encoded_bytes} fieldIdentity={JSON.stringify(field)}/>;
}

/** A deferred owner uses its advertised selected domain without inventing a Journal for inherited identity. */
export function AttemptIdentityValue({ field }: { field?: AttemptIdentitySource | null }) {
  if(!field)return null;
  return <NativeFieldValue domain={field.domain} path={field.type==='journal'?field.field.path:['attempt_id']}
    encodedBytes={field.type==='journal'?field.field.encoded_bytes:field.encoded_bytes} fieldIdentity={attemptIdentitySourceKey(field)}/>;
}

function NativeFieldValue({domain,path,encodedBytes,fieldIdentity}:{domain:string;path:string[];encodedBytes:number;fieldIdentity:string}) {
  const source = useContext(JournalSource), tx = useTranslation();
  const identity = JSON.stringify([observationSourceIdentity(source), fieldIdentity]);
  const currentIdentity = useRef(identity); currentIdentity.current = identity;
  const generation = useRef(0), decoder = useRef(new TextDecoder('utf-8',{ignoreBOM:true}));
  useEffect(() => () => { generation.current++; }, [identity]);
  const [state, setState] = useState<{ identity: string; cut?: CheckpointReadCut; next?: number; text?: string; pending?: boolean; error?: string }>();
  const loaded = state?.identity === identity ? state : undefined;
  const read = async (offset = 0) => {
    const token = ++generation.current, cut = offset === 0 ? source?.cut() : loaded?.cut;
    const current = () => generation.current === token && currentIdentity.current === identity && !!source?.owned();
    if (!source || !cut || !current()) return;
    if (!offset) decoder.current = new TextDecoder('utf-8',{ignoreBOM:true});
    setState({ identity, cut, pending: true });
    try {
      const { chunk } = await readObservationValue(source, cut, domain, path, offset, current);
      if (!current()) return;
      const encoded=chunk.bytes_base64;
      if (encoded.length>87_384 || encoded.length%4!==0 || encoded.length/4*3-(encoded.endsWith('==')?2:encoded.endsWith('=')?1:0)>65_536)
        throw new Error('Native journal chunk exceeds its byte bound.');
      const bytes = Uint8Array.from(atob(chunk.bytes_base64), character => character.charCodeAt(0)), end = offset + bytes.length;
      if (chunk.representation !== 'canonical_json_v1' || JSON.stringify(chunk.cut) !== JSON.stringify(cut) || chunk.domain !== domain || chunk.item_offset !== 0 || JSON.stringify(chunk.path) !== JSON.stringify(path)
        || chunk.offset !== offset || chunk.readable_end !== encodedBytes || !Number.isSafeInteger(end) || bytes.length > 65_536 || end > chunk.readable_end
        || offset < chunk.readable_end && !bytes.length || (chunk.next_offset ?? null) !== (end < chunk.readable_end ? end : null)) throw new Error('Native journal field range changed.');
      setState({ identity, cut, next: chunk.next_offset ?? undefined, text: decoder.current.decode(bytes, { stream: chunk.next_offset != null }) });
    } catch (error) { if (current()) setState({ identity, cut, error: String(error).slice(0, 512) }); }
  };
  return <section aria-label={tx('common:activity.content-deferred')}>
    <p role="status">{tx('common:activity.content-deferred')}</p>
    <Button disabled={!source?.owned() || loaded?.pending} onClick={() => void read()}>{tx('common:activity.read-value')}</Button>
    {loaded?.text && <pre>{loaded.text}</pre>}
    {loaded?.next != null && <Button disabled={loaded.pending || !source?.owned()} onClick={() => void read(loaded.next)}>{tx('common:activity.next-content')}</Button>}
    {loaded?.error && <p role="alert">{loaded.error}</p>}
  </section>;
}
