import {useCallback,useEffect,useMemo,useRef,useState} from 'react';
import type {CheckpointReadCut,TraceFieldSource} from '../../../../protocol/app-server/v48';
import {traceFieldKey} from '../../../../protocol/app-server/trace-identity';
import {readTraceFieldWindow} from '../../../../protocol/app-server/trace-field';
import type {TraceFieldReader} from '../../client/trace';
import {DeferredContent} from '../agent/DeferredContent';
import {Button} from '../../presentation/primitives/Button';
import {useTranslation} from '../../locale/react';

/** Explicit original ranges never overwrite a native alias, outcome or grouping. */
export function TraceFields({fields,cut,reader,owned}:{fields:readonly TraceFieldSource[];cut?:CheckpointReadCut;reader?:TraceFieldReader;owned:()=>boolean}) {
 const tx=useTranslation(),generation=useRef(0),decoder=useRef(new TextDecoder('utf-8',{ignoreBOM:true,fatal:true}));
 const identity=JSON.stringify([fields.map(traceFieldKey),cut]);
 const currentIdentity=useRef(identity);currentIdentity.current=identity;
 const latestOwned=useRef(owned),latestReader=useRef(reader);latestOwned.current=owned;latestReader.current=reader;
 const cutKey=JSON.stringify(cut),sourceCut=useMemo(()=>cut,[cutKey]);
 const ownsContent=useCallback(()=>!!sourceCut&&!!reader&&currentIdentity.current===identity&&latestReader.current===reader&&latestOwned.current(),[sourceCut,reader,identity]);
 const readContent=useCallback((reference:Parameters<TraceFieldReader['content']>[1],offset:number,current:()=>boolean)=>{
  if(!reader||!sourceCut)throw Error('Original Trace content source is unavailable.');
  return reader.content(sourceCut,reference,offset,current);
 },[reader,sourceCut]);
 useEffect(()=>()=>{generation.current++;},[identity,reader]);
 const [state,setState]=useState<{identity:string;reader?:TraceFieldReader;index:number;next?:number;text?:string;pending?:boolean;error?:string;representation?:string}>();
 const loaded=state?.identity===identity&&state.reader===reader?state:undefined;
 const [pageState,setPage]=useState<{identity:string;offset:number}>();
 const page=pageState?.identity===identity?pageState.offset:0;
 const read=async(index:number,offset=0)=>{
  const field=fields[index],token=++generation.current,current=()=>token===generation.current&&currentIdentity.current===identity&&latestReader.current===reader&&latestOwned.current();
  if(!field||field.type!=='value'||!current())return;
  if(!cut||!reader){setState({identity,reader,index,error:tx('trajectory:source.cut-unavailable')});return;}
  if(!offset)decoder.current=new TextDecoder('utf-8',{ignoreBOM:true,fatal:true});
  const representation=offset?loaded?.representation:undefined;
  setState({identity,reader,index,pending:true});
  try {
   const chunk=await readTraceFieldWindow(field,cut,offset,at=>reader.value(cut,field,at,current),current);
   if(!current())return;
   if(representation!=null&&representation!==chunk.representation)throw new Error('Trace field representation changed.');
   setState({identity,reader,index,next:chunk.next,representation:chunk.representation,text:decoder.current.decode(chunk.bytes,{stream:chunk.next!=null})});
  }catch(error){if(current())setState({identity,reader,index,error:String(error).slice(0,512)});}
 };
 if(!fields.length)return null;
 return <section aria-label={tx('common:activity.content-deferred')}>
  {fields.slice(page,page+32).map((field,row)=>field.type==='content'
   ?<div key={traceFieldKey(field)}><code>{field.field}</code><DeferredContent bindings={[field.binding]} owned={ownsContent} read={cut&&reader?readContent:undefined}/></div>
   :<div key={traceFieldKey(field)}><code>{field.field}</code> · {tx('trajectory:source.byte-count',{bytes:field.encoded_bytes})} <Button disabled={!cut||!reader||loaded?.pending||!owned()} onClick={()=>void read(page+row)}>{tx('common:activity.read-value')}</Button></div>)}
  {page+32<fields.length&&<Button onClick={()=>setPage({identity,offset:page+32})}>{tx('common:activity.next-content')}</Button>}
  {loaded?.text!=null&&<pre>{loaded.text}</pre>}
  {loaded?.next!=null&&<Button disabled={loaded.pending||!owned()} onClick={()=>void read(loaded.index,loaded.next)}>{tx('common:activity.next-content')}</Button>}
  {loaded?.error&&<p role="alert">{loaded.error}</p>}
  {!cut&&<p role="status">{tx('trajectory:source.cut-unavailable')}</p>}
 </section>;
}
