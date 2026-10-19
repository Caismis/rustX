import { useContext,useEffect,useRef,useState } from 'react';
import type { CheckpointReadCut,RuntimeClientDeliveryHead,RuntimeClientDeliveryItem } from '../../../../protocol/app-server/v52';
import { deliveryHead,deliveryItem,deliveryItemKey,deliveryPage,readDeliveryMetadata } from '../../../../protocol/app-server/delivery-observation';
import { observationSourceIdentity,readObservationValue,useObservationSource } from './JournalValue';
import { DeliveryCards } from '../components/Artifact';
import { PreviewContext } from '../components/ArtifactPreview';
import { Button } from '../../presentation/primitives/Button';
import { useTranslation } from '../../locale/react';

/** Positive successful C, independent of the execution's J settlement. */
export function DeliveryHead({head}:{head:RuntimeClientDeliveryHead}) {
 try{deliveryHead(head);}catch(error){return <p role="alert">{String(error).slice(0,512)}</p>;}
 return <NativeDeliveryHead head={head}/>;
}
function NativeDeliveryHead({head}:{head:RuntimeClientDeliveryHead}) {
 const source=useObservationSource(),preview=useContext(PreviewContext),tx=useTranslation();
 const identity=JSON.stringify([observationSourceIdentity(source),source&&'historyCut' in source?source.historyCut:undefined,head]);
 const currentIdentity=useRef(identity);currentIdentity.current=identity;
 const generation=useRef(0);useEffect(()=>()=>{generation.current++;},[identity]);
 const [state,setState]=useState<{identity:string;head?:RuntimeClientDeliveryHead;cut?:CheckpointReadCut;pending?:boolean;error?:string}>();
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
  try{const result=await page(shown.next_offset,loaded?.cut??source?.cut(),current);if(current())setState({identity,...result});}
  catch(error){if(current())setState({identity,head:shown,cut:loaded?.cut,error:String(error).slice(0,512)});}
 };
 const action=async(item:RuntimeClientDeliveryItem,download:boolean)=>{
  const token=++generation.current,current=()=>token===generation.current&&currentIdentity.current===identity&&!!source?.owned();if(!source||!preview||!current())return;
  setState({identity,head:shown,cut:loaded?.cut,pending:true});
  try{
   deliveryItem(item);let cut=loaded?.cut??source.cut();
   if(!cut)cut=(await page(0,undefined,current)).cut;
   const file=await readDeliveryMetadata(item,cut,async offset=>{
    if('historyCut' in source&&source.historyCut)return (await source.client.request({method:'session/historyValueRead',params:{session_id:source.sessionId,node_id:source.nodeId??null,cut:source.historyCut,domain:item.metadata_domain,item_offset:0,path:[],offset,max_bytes:65_536}},'checkpoint_value_chunk',undefined,current)).chunk;
    return (await readObservationValue(source,cut!,item.metadata_domain,[],offset,current)).chunk;
   },current);
   if(!current())return;
   const intent={source:{kind:'session_file' as const,conversationId:cut.conversation_id,owned:source.owned,locator:item.locator,index:item.index,file,agentId:'target' in source?source.agentId:undefined},name:file.name,image:file.mime_type.startsWith('image/'),mimeType:file.mime_type};
   if(download)preview.download(intent);else preview.openPreview(intent);
   setState({identity,head:shown,cut});
  }catch(error){if(current())setState({identity,head:shown,cut:loaded?.cut,error:String(error).slice(0,512)});}
 };
 return <section data-canonical-deliveries>
  {sourceCid&&<DeliveryCards conversationId={sourceCid} owned={source?.owned} deliveries={shown.items.flatMap(item=>item.file?[{key:deliveryItemKey(item),locator:item.locator,index:item.index,file:item.file}]:[])}/>}
  {shown.items.filter(item=>item.file==null).map(item=><div key={deliveryItemKey(item)} data-delivery-metadata-deferred><p>{tx('common:activity.content-deferred')} · {String(BigInt(item.index)+1n)}/{head.total}</p>
   <Button disabled={loaded?.pending||!preview||!source?.owned()} onClick={()=>void action(item,false)}>{tx('artifacts:artifact.preview')} {String(BigInt(item.index)+1n)}/{head.total}</Button>
   <Button disabled={loaded?.pending||!preview||!source?.owned()} onClick={()=>void action(item,true)}>{tx('artifacts:attachment-card.download')} {String(BigInt(item.index)+1n)}/{head.total}</Button></div>)}
  {shown.next_offset!=null&&<Button disabled={loaded?.pending||!source?.owned()} onClick={()=>void next()}>{tx('common:activity.next-page')}</Button>}
  {loaded?.error&&<p role="alert">{loaded.error}</p>}
 </section>;
}
