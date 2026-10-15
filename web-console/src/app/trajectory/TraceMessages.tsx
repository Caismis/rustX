import {useEffect,useRef,useState,type ReactNode} from 'react';
import type {CheckpointPage,TraceArtifact,TraceInspectionMessages,TraceInspectionMessage,TraceFieldSource} from '../../../../protocol/app-server/v48';
import {traceInspectionMessage} from '../../../../protocol/app-server/trace-inspection';
import {traceFieldKey} from '../../../../protocol/app-server/trace-identity';
import {canonicalLocatorKey} from '../../../../protocol/app-server/content-identity';
import type {TraceFieldReader} from '../../client/trace';
import {useTranslation} from '../../locale/react';
import {Button} from '../../presentation/primitives/Button';
import {TraceCollections} from './TraceCollections';
import {TraceFields} from './TraceFields';

const count=(value:unknown)=>typeof value==='string'&&/^(0|[1-9][0-9]{0,19})$/.test(value)&&BigInt(value)<=18446744073709551615n;
function selectedPage(page:CheckpointPage,original:CheckpointPage,domain:string,offset:number,total?:string) {
 if(page.domain!==domain||page.cursor!==page.read_cut.revision||JSON.stringify(page.read_cut)!==JSON.stringify(original.read_cut)||!count(page.total)||total!=null&&page.total!==total
  ||!Array.isArray(page.items)||page.items.length>32||new TextEncoder().encode(JSON.stringify(page)).length>786432)throw Error('Original Trace message page changed.');
 const end=offset+page.items.length,next=Number.isSafeInteger(end)&&BigInt(end)<BigInt(page.total)?String(end):null;
 if(!Number.isSafeInteger(offset)||offset<0||!Number.isSafeInteger(end)||BigInt(end)>BigInt(page.total)||end===offset&&BigInt(offset)<BigInt(page.total)||(page.next_offset??null)!==next||page.items.some((item,index)=>item.offset!==String(offset+index)))throw Error('Original Trace message page did not advance.');
 return page;
}
interface State {
 identity:string;reader?:TraceFieldReader;rows?:TraceInspectionMessage[];next?:number;message?:TraceInspectionMessage;
 pending?:boolean;error?:string;
}
/** One current native page and one selected original window; no assembled Message or Detail. */
export function TraceMessages({messages,page,reader,artifacts,sourceOnly=false,owned,fields:recordFields=[]}:{messages:TraceInspectionMessages;page:CheckpointPage;reader?:TraceFieldReader;artifacts:(items:TraceArtifact[])=>ReactNode;sourceOnly?:boolean;owned:()=>boolean;fields?:readonly TraceFieldSource[]}) {
 const tx=useTranslation(),identity=JSON.stringify([messages.domain,page.read_cut]),generation=useRef(0),latest=useRef(identity),latestReader=useRef(reader);
 latest.current=identity;latestReader.current=reader;
 const [state,setState]=useState<State>(),[selected,setSelected]=useState(0),[mode,setMode]=useState('fields'),[field,setField]=useState(0);
 const loaded=state?.identity===identity&&state.reader===reader?state:undefined;
 const rows=loaded?.rows??messages.items,message=loaded?.message??rows[selected]??rows[0],next=loaded?.message?undefined:loaded?.rows?loaded.next:messages.next_offset;
 const current=(token:number)=>token===generation.current&&latest.current===identity&&latestReader.current===reader&&owned();
 useEffect(()=>()=>{generation.current++;},[identity,reader]);
 const reset=()=>{generation.current++;setState(undefined);setSelected(0);setMode('fields');setField(0);};
 const load=async(domain:string,offset:number)=>{
  if(!reader||domain!==messages.domain)return;
  const token=++generation.current,isCurrent=()=>current(token);
  setState({identity,reader,pending:true});
  try{
   const result=selectedPage(await reader.page(page.read_cut,domain,offset,isCurrent),page,domain,offset,messages.total);
   if(!isCurrent())return;
    const rows=result.items.map(item=>{if(!item.deferred||item.body_complete!==false)throw Error('Original Trace message descriptor changed.');return traceInspectionMessage(item.value);});
    setState({identity,reader,rows,next:result.next_offset==null?undefined:Number(result.next_offset)});setSelected(0);setMode('fields');setField(0);
  }catch(error){if(isCurrent())setState({identity,reader,error:String(error).slice(0,512)});}
 };
 const fields=[...new Map([...recordFields,...message?.fields??[]].map(value=>[traceFieldKey(value),value])).values()],chosen=fields[field]??fields[0];
 const selectedIndex=rows[selected]?selected:0,fieldIndex=fields[field]?field:0;
 const activeMode=mode==='fields'||message?.collections.some(value=>value.domain===mode)?mode:'fields';
 return <section aria-label={tx('trajectory:group.message')}>
  <p>{messages.total}</p>
  <Button disabled={loaded?.pending||!owned()} onClick={reset}>{tx('common:activity.first-page')}</Button>
  {!loaded?.message&&rows.length>0&&<label>{tx('trajectory:group.message')} <select value={selectedIndex} onChange={event=>{generation.current++;setSelected(Number(event.target.value));setField(0);setMode('fields');setState(loaded?{identity,reader,rows:loaded.rows,next:loaded.next}:undefined);}}>{rows.map((row,index)=><option key={canonicalLocatorKey(row.locator)} value={index}>{row.identity??canonicalLocatorKey(row.locator)}</option>)}</select></label>}
  {message&&<>
   <code>{message.identity??canonicalLocatorKey(message.locator)}</code>
   {message.source!=null&&<pre>{JSON.stringify(message.source)}</pre>}
   {message.context_kind!=null&&<code>{message.context_kind}</code>}
   <p>{tx('trajectory:message.ordinary-input')}: {String(message.ordinary_input)}</p>
   {!sourceOnly&&<>
    {message.preview&&<pre>{message.preview.text}</pre>}{message.preview?.truncated&&<p role="status">{tx('common:activity.content-deferred')}</p>}
    {artifacts(message.attachments)}
    <label>{tx('common:activity.collection')} <select value={activeMode} onChange={event=>{generation.current++;setMode(event.target.value);setState({identity,reader,message});}}><option value="fields">{tx('common:activity.read-value')}</option>{message.collections.map(value=><option key={value.domain} value={value.domain}>{value.domain} ({value.total})</option>)}</select></label>
    {activeMode==='fields'?<>
     {fields.length>0&&<label>{tx('common:activity.read-value')} <select value={fieldIndex} onChange={event=>{generation.current++;setField(Number(event.target.value));}}>{fields.map((value,index)=><option key={index} value={index}>{value.field}</option>)}</select></label>}
     {chosen&&<TraceFields key={JSON.stringify([canonicalLocatorKey(message.locator),field])} fields={[chosen]} cut={page.read_cut} reader={reader} owned={()=>latest.current===identity&&latestReader.current===reader&&owned()}/>}
    </>:<>
     <TraceCollections collections={message.collections} domain={activeMode} page={page} reader={reader} owned={owned}/>
    </>}
   </>}
  </>}
  {next!=null&&<Button disabled={!reader||loaded?.pending||!owned()} onClick={()=>void load(messages.domain,next)}>{tx('common:activity.next-page')}</Button>}
  {loaded?.error&&<p role="alert">{loaded.error}</p>}
 </section>;
}
