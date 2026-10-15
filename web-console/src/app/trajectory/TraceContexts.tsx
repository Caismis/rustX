import {useEffect,useRef,useState} from 'react';
import type {CheckpointReadCut,TraceContextPresentation,TraceRequestSummary} from '../../../../protocol/app-server/v48';
import {canonicalLocatorKey} from '../../../../protocol/app-server/content-identity';
import type {TraceFieldReader} from '../../client/trace';
import {TraceFields} from './TraceFields';
import {Button} from '../../presentation/primitives/Button';
import {useTranslation} from '../../locale/react';

/** One selected native context page, replaced on continuation rather than accumulated. */
export function TraceContexts({request,cut,reader,owned}:{request:TraceRequestSummary;cut?:CheckpointReadCut;reader?:TraceFieldReader;owned:()=>boolean}) {
 const tx=useTranslation(),generation=useRef(0),identity=JSON.stringify([request.context_domain,request.context_total,cut]);
 const latest=useRef(identity);latest.current=identity;
 useEffect(()=>()=>{generation.current++;},[identity,reader]);
 const [state,setState]=useState<{identity:string;reader?:TraceFieldReader;rows:TraceContextPresentation[];next?:number;pending?:boolean;error?:string}>();
 const loaded=state?.identity===identity&&state.reader===reader?state:undefined;
 const read=async(offset:number)=>{
  const token=++generation.current,current=()=>token===generation.current&&latest.current===identity&&owned();
  if(!cut||!reader||!current())return;
  setState({identity,reader,rows:[],pending:true});
  try{
   const raw=request.context_total;
   if(typeof raw!=='string'||raw.length>20||!/^(0|[1-9][0-9]*)$/.test(raw)||BigInt(raw)>18446744073709551615n||!Number.isSafeInteger(offset)||offset<0)throw new Error('Original Context coordinates are unavailable.');
   const page=await reader.page(cut,request.context_domain,offset,current);
   if(!current())return;
   if(!Array.isArray(page.items)||page.items.length>32||JSON.stringify(page.read_cut)!==JSON.stringify(cut)||page.domain!==request.context_domain||page.total!==raw)throw new Error('Original Context page changed.');
   const end=offset+page.items.length,next=BigInt(end)<BigInt(raw)?end:undefined;
   if(!Number.isSafeInteger(end)||BigInt(end)>BigInt(raw)||BigInt(offset)<BigInt(raw)&&!page.items.length||(page.next_offset??null)!==(next==null?null:String(next)))throw new Error('Original Context page did not advance.');
   const rows=page.items.map((item,index)=>{
    if(item.offset!==String(offset+index)||item.deferred||!item.value||typeof item.value!=='object'||Array.isArray(item.value))throw new Error('Original Context header is unavailable.');
    const row=item.value as unknown as TraceContextPresentation;
    if(!row.locator||!Array.isArray(row.fields))throw new Error('Original Context header identity is unavailable.');
    canonicalLocatorKey(row.locator);return row;
   });
   setState({identity,reader,rows,next});
  }catch(error){if(current())setState({identity,reader,rows:[],error:String(error).slice(0,512)});}
 };
 if(request.context_next_offset==null)return null;
 return <section aria-label={tx('common:activity.content-deferred')}>
  <span>{request.context_total}</span> · <Button disabled={!cut||!reader||loaded?.pending||!owned()} onClick={()=>void read(request.context_next_offset!)}>{tx('common:activity.read-value')}</Button>
  {loaded?.rows.map(row=><div key={canonicalLocatorKey(row.locator)}><p>{row.preview?.text}</p><TraceFields fields={row.fields} cut={cut} reader={reader} owned={()=>latest.current===identity&&owned()}/></div>)}
  {loaded?.next!=null&&<Button disabled={loaded.pending||!owned()} onClick={()=>void read(loaded.next!)}>{tx('common:activity.next-content')}</Button>}
  {loaded?.error&&<p role="alert">{loaded.error}</p>}
 </section>;
}
