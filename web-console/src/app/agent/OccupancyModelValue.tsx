import { useEffect, useRef, useState } from 'react';
import type { CheckpointReadCut, RuntimeClientContextOccupancy } from '../../../../protocol/app-server/v48';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { observationSourceIdentity, readObservationValue, useObservationSource } from './JournalValue';

/** Explicit original model-field ranges; no model identity is reconstructed. */
export function OccupancyModelValue({ occupancy }: { occupancy: RuntimeClientContextOccupancy }) {
  const source = useObservationSource(), tx = useTranslation(), scope = source?.cut();
  const identity = JSON.stringify([observationSourceIdentity(source), scope?.conversation_id, scope?.activation_id, occupancy.model_source]);
  const currentIdentity = useRef(identity); currentIdentity.current = identity;
  const generation = useRef(0), decoder = useRef(new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}));
  useEffect(() => () => { generation.current++; }, [identity]);
  const [state,setState] = useState<{identity:string;cut:CheckpointReadCut;next?:number;text?:string;pending?:boolean;error?:string}>();
  const loaded = state?.identity === identity ? state : undefined;
  if (!occupancy.model_deferred) return null;
  const read = async (offset = 0) => {
    const token = ++generation.current, cut = offset === 0 ? source?.cut() : loaded?.cut, domain = occupancy.model_source;
    const current = () => generation.current === token && currentIdentity.current === identity && !!source?.owned();
    if (!source || !cut || !domain || !current()) return;
    if (!offset) decoder.current = new TextDecoder('utf-8',{fatal:true,ignoreBOM:true});
    setState({identity,cut,pending:true});
    try {
      const {chunk} = await readObservationValue(source, cut, domain, [], offset, current);
      if (!current()) return;
      const encoded = chunk.bytes_base64;
      if (encoded.length>87_384 || encoded.length%4!==0 || encoded.length/4*3-(encoded.endsWith('==')?2:encoded.endsWith('=')?1:0)>65_536)
        throw new Error('Encoded native range exceeds the decoded byte bound.');
      const bytes = Uint8Array.from(atob(encoded),value=>value.charCodeAt(0)), end=offset+bytes.length;
      if (chunk.representation!=='canonical_json_v1' || JSON.stringify(chunk.cut)!==JSON.stringify(cut)
        || chunk.domain!==domain || chunk.item_offset!==0 || chunk.path.length!==0 || chunk.offset!==offset
        || !Number.isSafeInteger(chunk.readable_end) || !Number.isSafeInteger(end) || end>chunk.readable_end
        || bytes.length>65_536 || offset<chunk.readable_end && bytes.length===0
        || (chunk.next_offset??null)!==(end<chunk.readable_end?end:null)) throw new Error('Original context model range changed.');
      setState({identity,cut,next:chunk.next_offset??undefined,text:decoder.current.decode(bytes,{stream:chunk.next_offset!=null})});
    } catch (error) { if (current()) setState({identity,cut,error:String(error).slice(0,512)}); }
  };
  return <section aria-label={tx('agent:usage.model')}>
    <p role="status">{tx('agent:usage.model')}: {tx('common:activity.content-deferred')}</p>
    <Button disabled={!occupancy.model_source || !source?.owned() || loaded?.pending} onClick={()=>void read()}>{tx('common:activity.read-value')}</Button>
    {loaded?.text!=null && <pre>{loaded.text}</pre>}
    {loaded?.next!=null && <Button disabled={loaded.pending || !source?.owned()} onClick={()=>void read(loaded.next)}>{tx('common:activity.next-content')}</Button>}
    {loaded?.error && <p role="alert">{loaded.error}</p>}
  </section>;
}
