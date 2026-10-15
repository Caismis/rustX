import {useEffect,useRef,useState} from 'react';
import type {CheckpointPage,CheckpointPageItem} from '../../../../protocol/app-server/v48';
import {readTraceSelectedWindow,type TraceValueSelector} from '../../../../protocol/app-server/trace-field';
import type {TraceFieldReader} from '../../client/trace';
import {useTranslation} from '../../locale/react';
import {Button} from '../../presentation/primitives/Button';

export const REQUEST_OPTION_FIELDS=['model','protocol','reasoning_enabled','profile','max_output_tokens','context_window_tokens','options','contributions'] as const;
export type RequestField=typeof REQUEST_OPTION_FIELDS[number]|'effective_system_prompt'|'previous_system_prompt'|'tools';
const FIELD_LABEL={model:'details.model',protocol:'field.protocol',reasoning_enabled:'field.reasoning',profile:'field.profile',max_output_tokens:'field.max-output',context_window_tokens:'field.context-window',options:'tab.options',contributions:'field.contributions',effective_system_prompt:'field.current-prompt',previous_system_prompt:'field.previous-prompt',tools:'tab.tools'} as const;
function selectedPage(page:CheckpointPage,original:CheckpointPage,domain:string,offset:number) {
 if(!Array.isArray(page.items)||page.items.length>32||page.domain!==domain||page.cursor!==page.read_cut.revision||JSON.stringify(page.read_cut)!==JSON.stringify(original.read_cut)
  ||typeof page.total!=='string'||!/^(0|[1-9][0-9]{0,19})$/.test(page.total)||BigInt(page.total)>18446744073709551615n)throw Error('Original request field page changed.');
 const total=BigInt(page.total),end=offset+page.items.length,next=Number.isSafeInteger(end)&&BigInt(end)<total?String(end):null;
 if(!Number.isSafeInteger(offset)||offset<0||!Number.isSafeInteger(end)||BigInt(end)>total||end===offset&&BigInt(offset)<total||(page.next_offset??null)!==next
  ||page.items.some((item,index)=>item.offset!==String(offset+index)))throw Error('Original request field page did not advance.');
 return page;
}

/** One selected child page and one original byte window, never an assembled request. */
export function TraceRequestFields({page,reader,fields,previousDomain,onEarlier}:{page:CheckpointPage;reader?:TraceFieldReader;fields:readonly RequestField[];previousDomain?:string;onEarlier?:()=>void}) {
 const tx=useTranslation(),[selection,setSelection]=useState<RequestField>(fields[0]),generation=useRef(0);
 const field=fields.includes(selection)?selection:fields[0],domain=field==='previous_system_prompt'?previousDomain:`${page.domain}.request.${field}`;
 const identity=JSON.stringify([page.domain,page.read_cut,field,domain]),latest=useRef(identity);latest.current=identity;
 const decoder=useRef(new TextDecoder('utf-8',{fatal:true,ignoreBOM:true}));
 const [state,setState]=useState<{identity:string;reader?:TraceFieldReader;head?:CheckpointPage;pending?:boolean;error?:string;window?:{selector:TraceValueSelector;text:string;next?:number;end:number;representation:string}}>();
 const loaded=state?.identity===identity&&state.reader===reader?state:undefined;
 useEffect(()=>()=>{generation.current++;},[identity,reader]);
 const current=(token:number)=>token===generation.current&&latest.current===identity;
 const window=async(selector:TraceValueSelector,offset=0)=>{
  if(!reader)return;
  const token=++generation.current,owned=()=>current(token);
  const previous=offset?loaded?.window:undefined;
  if(offset&&(!previous||JSON.stringify(previous.selector)!==JSON.stringify(selector)))return;
  if(!offset)decoder.current=new TextDecoder('utf-8',{fatal:true,ignoreBOM:true});
  setState({identity,reader,head:loaded?.head,pending:true});
  try{
   const chunk=await readTraceSelectedWindow(selector,page.read_cut,offset,at=>reader.value(page.read_cut,selector,at,owned),owned,previous?.end);
   if(previous&&previous.representation!==chunk.representation)throw Error('Original request representation changed.');
   if(owned())setState({identity,reader,head:loaded?.head,window:{selector,text:decoder.current.decode(chunk.bytes,{stream:chunk.next!=null}),next:chunk.next,end:chunk.readable_end,representation:chunk.representation}});
  }catch(error){if(owned())setState({identity,reader,head:loaded?.head,error:String(error).slice(0,512)});}
 };
 const load=async(offset=0)=>{
  if(!reader||!domain||!/^trace\.records\.(0|[1-9][0-9]*)\.detail$/.test(page.domain)||fields.length>8)return;
  const token=++generation.current,owned=()=>current(token);
  setState({identity,reader,pending:true});
  try{
   const head=selectedPage(await reader.page(page.read_cut,domain,offset,owned),page,domain,offset);
   if(!owned())return;
   if(head.total==='0'){
    // Scalars have no children; their first authoritative range supplies H.
    const selector={domain,item_offset:0,path:[]};
    decoder.current=new TextDecoder('utf-8',{fatal:true,ignoreBOM:true});
    const chunk=await readTraceSelectedWindow(selector,page.read_cut,0,at=>reader.value(page.read_cut,selector,at,owned),owned);
    if(owned())setState({identity,reader,window:{selector,text:decoder.current.decode(chunk.bytes,{stream:chunk.next!=null}),next:chunk.next,end:chunk.readable_end,representation:chunk.representation}});
   }else setState({identity,reader,head});
  }catch(error){if(owned())setState({identity,reader,error:String(error).slice(0,512)});}
 };
 const item=(row:CheckpointPageItem)=><div key={row.identity}>
  <code>{row.key?.preview??row.identity}</code>{row.deferred&&<span> · {tx('common:activity.content-deferred')}</span>}
  <pre>{JSON.stringify(row.value)}</pre>
  <Button disabled={!reader||loaded?.pending} onClick={()=>void window({domain:loaded!.head!.domain,item_offset:Number(row.offset),path:[]})}>{tx('common:activity.read-value')}</Button>
 </div>;
 return <section aria-label={tx('trajectory:options.json')}>
  <select aria-label={tx('trajectory:options.json')} value={field} onChange={event=>{generation.current++;setSelection(event.target.value as RequestField);}}>{fields.map(name=><option key={name} value={name}>{tx(`trajectory:${FIELD_LABEL[name]}`)}</option>)}</select>
  <Button disabled={!reader||!domain||loaded?.pending} onClick={()=>void load()}>{tx('common:activity.read-value')}</Button>
  {!domain&&field==='previous_system_prompt'?<p role="status">{tx('trajectory:field.previous-not-loaded')}{onEarlier&&<> <Button onClick={onEarlier}>{tx('trajectory:history.load-earlier')}</Button></>}</p>:!reader&&<p role="status">{tx('trajectory:source.cut-unavailable')}</p>}
  {loaded?.head?.items.map(item)}
  {loaded?.head?.next_offset!=null&&<Button disabled={loaded.pending} onClick={()=>void load(Number(loaded.head!.next_offset))}>{tx('common:activity.next-content')}</Button>}
  {loaded?.window&&<><pre>{loaded.window.text}</pre>{loaded.window.next!=null&&<Button disabled={loaded.pending} onClick={()=>void window(loaded.window!.selector,loaded.window!.next!)}>{tx('common:activity.next-content')}</Button>}</>}
  {loaded?.error&&<p role="alert">{loaded.error}</p>}
 </section>;
}
