import { useEffect,useRef,useState } from 'react';
import type { CheckpointReadCut,RuntimeClientDeliveryHead,RuntimeClientDeliveryItem,RuntimeClientMessageDescriptor } from '../../../../protocol/app-server/v57';
import { deliveryHead,deliveryItem,deliveryItemKey,deliveryPage,readDeliveryMetadata } from '../../../../protocol/app-server/delivery-observation';
import { observationSourceIdentity,readObservationValue,useObservationSource } from './ObservationSource';
import { DeliveryCards } from '../components/Artifact';
import { Button } from '../../presentation/primitives/Button';
import { useTranslation } from '../../locale/react';

/** Positive successful C, independent of the execution's J settlement. */
type DeliveryHeadProps = {head:RuntimeClientDeliveryHead;messageIdentity:RuntimeClientMessageDescriptor['identity']};
export function DeliveryHead({head,messageIdentity}:DeliveryHeadProps) {
 try{deliveryHead(head);}catch(error){return <p role="alert">{String(error).slice(0,512)}</p>;}
 return <NativeDeliveryHead head={head} messageIdentity={messageIdentity}/>;
}
type DeliveryView = Omit<RuntimeClientDeliveryHead,'items'> & {items:RuntimeClientDeliveryItem[]};
function NativeDeliveryHead({head,messageIdentity}:DeliveryHeadProps) {
 const source=useObservationSource(),tx=useTranslation();
 const identity=JSON.stringify([observationSourceIdentity(source),source&&'historyCut' in source?source.historyCut:undefined,messageIdentity,head]);
 const currentIdentity=useRef(identity);currentIdentity.current=identity;
 const generation=useRef(0);useEffect(()=>()=>{generation.current++;},[identity]);
 const [state,setState]=useState<{identity:string;head?:DeliveryView;cut?:CheckpointReadCut;pending?:boolean;error?:string}>();
 const loaded=state?.identity===identity?state:undefined,shown=loaded?.head??head;
 const sourceCid=source&&'target' in source?source.cut()?.conversation_id:source&&'historyCut' in source?source.historyCut?.read.conversation_id:source?.cut()?.conversation_id;
 const page=async(offset:number,cut:CheckpointReadCut|undefined,current:()=>boolean)=>{
  if(!source)throw new Error('Native delivery source is unavailable.');
  const captured=cut??source.cut();
  if(!captured && ('target' in source || !source.historyCut))throw new Error('Native delivery source cut is unavailable.');
  const result='target' in source
   ? await source.client.request({method:'conversation/checkpointPage',params:{target:source.target,agent_id:source.agentId??null,cut:captured!,domain:head.domain,offset,limit:8}},'checkpoint_page',undefined,current)
   : source.historyCut
    ? await source.client.request({method:'session/historyPage',params:{session_id:source.sessionId,node_id:source.nodeId??null,cut:source.historyCut,domain:head.domain,offset,limit:8}},'checkpoint_page',undefined,current)
    : await source.client.request({method:'session/tracePage',params:{session_id:source.sessionId,node_id:source.nodeId??null,cut:captured!,domain:head.domain,offset,limit:8}},'checkpoint_page',undefined,current);
  if(!current())throw new Error('Native delivery view changed.');
  const actual=result.page.read_cut;
  if(actual.conversation_id!==sourceCid||cut&&JSON.stringify(actual)!==JSON.stringify(cut)||'historyCut' in source&&source.historyCut&&JSON.stringify(actual.source_cut)!==JSON.stringify(source.historyCut))throw new Error('Native delivery source cut changed.');
  return {head:deliveryPage(head,result.page,actual,offset),cut:actual};
 };
 const next=async()=>{const token=++generation.current,current=()=>token===generation.current&&currentIdentity.current===identity&&!!source?.owned();if(!current()||shown.next_offset==null)return;
  setState({identity,head:shown,cut:loaded?.cut,pending:true});
  try{const result=await page(shown.next_offset,loaded?.cut??source?.cut(),current);if(current())setState({identity,...result,head:{...result.head,items:[...shown.items,...result.head.items]}});}
  catch(error){if(current())setState({identity,head:shown,cut:loaded?.cut,error:String(error).slice(0,512)});}
 };
 const missing=shown.items.filter(item=>item.file==null),missingKey=JSON.stringify(missing),active=!!source?.owned();
 useEffect(()=>{
  if(!source||!active||!missing.length)return;
  let mounted=true;const token=++generation.current,current=()=>mounted&&token===generation.current&&currentIdentity.current===identity&&source.owned();
  setState({identity,head:shown,cut:loaded?.cut,pending:true});
  void(async()=>{
   let cut=loaded?.cut??source.cut();if(!cut)cut=(await page(0,undefined,current)).cut;
   const files=new Map<string,NonNullable<RuntimeClientDeliveryItem['file']>>();
   for(const item of missing){
    deliveryItem(item);
    const file=await readDeliveryMetadata(item,cut,async offset=>{
     if('historyCut' in source&&source.historyCut)return (await source.client.request({method:'session/historyValueRead',params:{session_id:source.sessionId,node_id:source.nodeId??null,cut:source.historyCut,domain:item.metadata_domain,item_offset:0,path:[],offset,max_bytes:65_536}},'checkpoint_value_chunk',undefined,current)).chunk;
     return (await readObservationValue(source,cut!,item.metadata_domain,[],offset,current)).chunk;
    },current);
    if(!current())return;files.set(deliveryItemKey(item),file);
   }
   if(current())setState({identity,cut,head:{...shown,items:shown.items.map(item=>files.has(deliveryItemKey(item))?{...item,file:files.get(deliveryItemKey(item)),metadata_deferred:false}:item)}});
  })().catch(error=>{if(current())setState({identity,head:shown,cut:loaded?.cut,error:String(error).slice(0,512)});});
  return()=>{mounted=false;};
 },[identity,missingKey,source?.client,active]);
 // The native descriptor exposes only bounded message IDs. Use that same
 // identity before and after transcript hydration; oversized IDs stay ledger-based.
 return <section data-canonical-deliveries>
  {sourceCid&&<DeliveryCards conversationId={sourceCid} owned={source?.owned} deliveries={shown.items.flatMap(item=>item.file?[{key:deliveryItemKey(item),locator:messageIdentity==null?item.locator:{type:'by_message_id',message_id:messageIdentity},index:item.index,file:item.file}]:[])}/>}
  {shown.next_offset!=null&&<Button disabled={loaded?.pending||!source?.owned()} onClick={()=>void next()}>{tx('common:activity.show-more')}</Button>}
  {loaded?.error&&<p role="alert">{loaded.error}</p>}
 </section>;
}
