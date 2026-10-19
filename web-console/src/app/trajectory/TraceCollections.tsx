import {useCallback,useEffect,useMemo,useRef,useState} from 'react';
import type {CheckpointPage,CollectionFrontier} from '../../../../protocol/app-server/v52';
import {validateCanonicalInspectionPage} from '../../../../protocol/app-server/trace-inspection';
import type {TraceFieldReader} from '../../client/trace';
import {useTranslation} from '../../locale/react';
import {Button} from '../../presentation/primitives/Button';
import {DeferredContent} from '../agent/DeferredContent';

/** One original canonical page and selected ContentRef window, never a whole-item ValueRead. */
export function TraceCollections({collections,page,reader,owned,domain}:{collections:readonly CollectionFrontier[];page:CheckpointPage;reader?:TraceFieldReader;owned:()=>boolean;domain?:string}) {
 const tx=useTranslation(),[choice,setChoice]=useState(''),[itemIndex,setItem]=useState(0),[source,setSource]=useState(0),generation=useRef(0);
 const selected=domain??choice,frontier=collections.find(value=>value.domain===selected),cutKey=JSON.stringify(page.read_cut),cut=useMemo(()=>page.read_cut,[cutKey]);
 const identity=JSON.stringify([selected,frontier?.total,cutKey]),latest=useRef(identity),latestReader=useRef(reader),latestOwned=useRef(owned);
 latest.current=identity;latestReader.current=reader;latestOwned.current=owned;
 const current=useCallback(()=>latest.current===identity&&latestReader.current===reader&&latestOwned.current(),[identity,reader]);
 const content=useCallback((reference:Parameters<TraceFieldReader['content']>[1],offset:number,owned:()=>boolean)=>{
  if(!reader)throw Error('Original Trace content source is unavailable.');
  return reader.content(cut,reference,offset,owned);
 },[reader,cut]);
 const [state,setState]=useState<{identity:string;reader?:TraceFieldReader;page?:CheckpointPage;pending?:boolean;error?:string}>();
 const loaded=state?.identity===identity&&state.reader===reader?state:undefined;
 useEffect(()=>()=>{generation.current++;},[identity,reader]);
 const load=async(offset=0)=>{
  if(!reader||!frontier||!current())return;
  const token=++generation.current,owns=()=>token===generation.current&&current();
  setState({identity,reader,pending:true});
  try{
   const next=await reader.page(cut,frontier.domain,offset,owns);
   if(!owns())return;
   if(next.domain!==frontier.domain||next.cursor!==next.read_cut.revision||JSON.stringify(next.read_cut)!==cutKey||new TextEncoder().encode(JSON.stringify(next)).length>786432)throw Error('Original canonical page changed.');
   validateCanonicalInspectionPage(next,offset,frontier.total,offset!==0);
   setState({identity,reader,page:next});setItem(0);setSource(0);
  }catch(error){if(owns())setState({identity,reader,error:String(error).slice(0,512)});}
 };
 const item=loaded?.page?.items[itemIndex],binding=item?.content[source]??item?.content[0];
 return <section aria-label={tx('common:activity.collection')}>
  {domain==null&&<label>{tx('common:activity.collection')} <select value={frontier?selected:''} onChange={event=>{generation.current++;setChoice(event.target.value);setItem(0);setSource(0);}}><option value=""/>{collections.map(value=><option key={value.domain} value={value.domain}>{value.domain} ({value.total})</option>)}</select></label>}
  {frontier&&<Button disabled={!reader||loaded?.pending||!current()} onClick={()=>void load()}>{tx('common:activity.first-page')}</Button>}
  {loaded?.page&&<>
   <label>{tx('common:activity.collection')} <select value={itemIndex} onChange={event=>{generation.current++;setItem(Number(event.target.value));setSource(0);}}>{loaded.page.items.map((row,index)=><option key={row.offset} value={index}>{row.key?.preview??row.identity}</option>)}</select></label>
   {item&&<>{item.value!=null&&<pre>{JSON.stringify(item.value)}</pre>}{item.content.length>0&&<label>{tx('common:activity.read-value')} <select value={item.content[source]?source:0} onChange={event=>{generation.current++;setSource(Number(event.target.value));}}>{item.content.map((binding,index)=><option key={index} value={index}>{binding.path.join('.')} · {binding.content_ref.representation}</option>)}</select></label>}
    {binding&&<DeferredContent key={JSON.stringify([item.offset,source])} explicit bindings={[binding]} owned={current} read={reader?content:undefined}/>}</>}
   {loaded.page.next_offset!=null&&<Button disabled={loaded.pending||!current()} onClick={()=>void load(Number(loaded.page!.next_offset))}>{tx('common:activity.next-page')}</Button>}
  </>}
  {loaded?.error&&<p role="alert">{loaded.error}</p>}
 </section>;
}
