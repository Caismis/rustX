import { useEffect, useMemo, useRef, useState } from 'react';
import type { AttachmentTarget, ContentBinding, ContentRef, ContentChunk } from '../../../../protocol/app-server/v50';
import { ContentCache } from '../../../../protocol/app-server/content-cache';
import { contentRefKey } from '../../../../protocol/app-server/content-identity';
import type { AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { useTranslation } from '../../locale/react';

export function DeferredContent({ client, target, agentId, bindings, owned, read, explicit = false }: { client?: AppServerClient; target?: AttachmentTarget; agentId?: string; bindings: ContentBinding[]; explicit?: boolean; owned: () => boolean; read?:(ref:ContentRef,offset:number,current:()=>boolean)=>Promise<ContentChunk> }) {
  const tx = useTranslation();
  const generation = useRef(0), latestOwned = useRef(owned);latestOwned.current=owned;
  const identity = JSON.stringify(bindings.map(binding => [binding.path,binding.source_path,contentRefKey(binding.content_ref)]));
  const currentIdentity = useRef(identity); currentIdentity.current = identity;
  const cache = useMemo(() => new ContentCache((ref,offset,_max,current)=>{if(read)return read(ref,offset,current);if(!client||!target)throw new Error('Content source is unavailable.');return client.reads.readContent(target,agentId,ref,offset,current);}), [client,target,agentId,read]);
  useEffect(() => () => { generation.current++; cache.clear(); }, [cache, identity]);
  const [range, setRange] = useState<{ identity:string; cache:ContentCache; index: number; offset: number; next?: number; text?: string; loading?: boolean; error?: string }>();
  const reading=range?.identity===identity&&range.cache===cache?range:undefined;
  const setReading=(value:Omit<NonNullable<typeof range>,'identity'|'cache'>)=>setRange({...value,identity,cache});
  const decoder = useRef(new TextDecoder('utf-8',{ignoreBOM:true}));
  const [pageState,setPageState]=useState<{identity:string;cache:ContentCache;page:number}>();
  const page=pageState?.identity===identity&&pageState.cache===cache?pageState.page:0;
  const setPage=(page:number)=>setPageState({identity,cache,page});
  const deferred = bindings.filter(binding => explicit || binding.deferred);
  if (!deferred.length) return null;
  const load = async (index: number, offset = 0) => {
    const captured = ++generation.current, binding = deferred[index];
    if (!binding || !latestOwned.current()) return;
    if (offset === 0) decoder.current = new TextDecoder('utf-8',{ignoreBOM:true});
    setReading({ index, offset, loading: true });
    try {
      const chunk = await cache.read(binding.content_ref, offset, () => captured === generation.current && identity === currentIdentity.current && latestOwned.current());
      if (captured !== generation.current || identity !== currentIdentity.current || !latestOwned.current()) return;
      const binary=binding.content_ref.representation==='binary_bytes';
      const text=binary?btoa(String.fromCharCode(...chunk.bytes)):decoder.current.decode(chunk.bytes,{stream:chunk.next!==undefined});

      setReading({ index, offset, next: chunk.next, text });
    } catch (error) { if (captured === generation.current && identity === currentIdentity.current && latestOwned.current()) setReading({ index, offset, error: String(error).slice(0,512) }); }
  };
  return <section aria-label={tx('common:activity.content-deferred')}>
    {deferred.some(binding=>binding.deferred)&&<p role="status">{tx('common:activity.content-deferred')}</p>}
    {deferred.slice(page*32,(page+1)*32).map((binding, row) => <Button key={JSON.stringify([binding.path,binding.source_path,contentRefKey(binding.content_ref)])} disabled={reading?.loading} onClick={() => void load(page*32+row)}>{tx('common:activity.load-content', { bytes: binding.content_ref.readable_end })}</Button>)}
    {(page+1)*32<deferred.length&&<Button disabled={reading?.loading} onClick={()=>setPage(page+1)}>{tx('common:activity.next-content')}</Button>}
    {reading?.text && <pre>{reading.text}</pre>}
    {reading?.error && <p role="alert">{reading.error}</p>}
    {reading?.next !== undefined && <Button disabled={reading.loading} onClick={() => void load(reading.index, reading.next)}>{tx('common:activity.next-content')}</Button>}
  </section>;
}
