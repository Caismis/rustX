import { useEffect, useMemo, useRef, useState } from 'react';
import type { AttachmentTarget, CheckpointPage, CheckpointPageItem, CheckpointValueChunk, CheckpointReadCut, CollectionFrontier, RuntimeClientAgent } from '../../../../protocol/app-server/v52';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';

export interface CollectionRead {
  identity:string;
  page(domain:string,offset:number,previous:CheckpointPage|undefined,current:()=>boolean):Promise<CheckpointPage>;
  value(page:CheckpointPage,item:CheckpointPageItem,offset:number,current:()=>boolean):Promise<CheckpointValueChunk>;
}

/** A user-selected native page replaces its predecessor; it never changes Applied. */
export function CollectionPages({client,target,agentId,cut,read,collections,owned,onAgent,onModel}:{client?:AppServerClient;target?:AttachmentTarget;agentId?:string;cut?:CheckpointReadCut;read?:CollectionRead;collections:CollectionFrontier[];owned:()=>boolean;onAgent?:(agent:RuntimeClientAgent)=>void;onModel?:(page:CheckpointPage,item:CheckpointPageItem,current:()=>boolean)=>Promise<void>}) {
  const tx=useTranslation();
  const [selected,setSelected]=useState<string>();
  const frontier=collections.find(value=>value.domain===selected);
  const owner=selected?.split('.')[0] ?? '';
  const identity=JSON.stringify([read?.identity,target,agentId,selected,cut?.domain_revisions[owner],cut?.collection_cuts?.[owner],owner==='attempt'?cut?.live_publication:undefined,owner==='attempt'?cut?.canonical_foreground:undefined,owner==='messages'||owner==='transcript'?cut?.source_cut:undefined]);
  const currentIdentity=useRef(identity);currentIdentity.current=identity;
  const generation=useRef(0);
  useEffect(()=>()=>{generation.current++;},[identity]);
  const [state,setState]=useState<{identity:string;page?:CheckpointPage;loading?:boolean;error?:string;item?:string;offset?:number;next?:number;text?:string}>();
  const loaded=state?.identity===identity?state:undefined;
  const reader=useMemo(()=>client&&target?client.domainReader(target,()=>currentIdentity.current===identity&&owned(),agentId):undefined,[client,target,agentId,identity,owned]);
  const loadPage=async(offset:number)=>{
    const token=++generation.current, current=()=>token===generation.current&&currentIdentity.current===identity&&owned();
    if(!frontier||!current())return;
    setState({identity,page:loaded?.page,loading:true});
    try {
      if(!read&&(!client||!target||!cut))throw new Error('Collection source is unavailable.');
      const expectedCut=loaded?.page?.read_cut??cut;
      const page=read?await read.page(frontier.domain,offset,loaded?.page,current):(await client!.request({method:'conversation/checkpointPage',params:{target:target!,agent_id:agentId??null,cut:(loaded?.page?.read_cut??cut)!,domain:frontier.domain,offset,limit:32}},'checkpoint_page',undefined,current)).page;
      if(!current())return;
      const older = page.domain === 'transcript.entries' || page.domain === 'trace.records';
      const next = page.next_offset == null ? undefined : Number(page.next_offset);
      if(expectedCut&&JSON.stringify(page.read_cut)!==JSON.stringify(expectedCut)||page.domain!==frontier.domain||page.items.length>32||page.items.some(item=>{
        const at=Number(item.offset);
        return !Number.isSafeInteger(at)||at<0||(older ? at===0||offset!==0&&at>=offset : at<offset);
      })||next!==undefined&&(!Number.isSafeInteger(next)||!page.items.length||(older ? next<=0||offset!==0&&next>=offset : next<=offset)))throw new Error('Native collection page did not advance.');
      setState({identity,page});
    }catch(error){if(current())setState({identity,page:loaded?.page,error:String(error).slice(0,512)});}
  };
  const decoder=useRef(new TextDecoder('utf-8',{ignoreBOM:true}));
  const readValue=async(item:NonNullable<CheckpointPage['items']>[number],offset=0)=>{
    const token=++generation.current,current=()=>token===generation.current&&currentIdentity.current===identity&&owned(),page=loaded?.page;
    if(!page||!current())return;
    if(offset===0)decoder.current=new TextDecoder('utf-8',{ignoreBOM:true});
    setState({identity,page,item:item.identity,offset,loading:true});
    try {
      const chunk=read?await read.value(page,item,offset,current):reader?await reader.value(page,item,offset):undefined;
      if(!chunk)throw new Error('Collection source is unavailable.');
      if(chunk.bytes_base64.length>87_384)throw new Error('Native collection chunk exceeds its byte bound.');
      if(!current())return;
      const bytes=Uint8Array.from(atob(chunk.bytes_base64),value=>value.charCodeAt(0)),end=offset+bytes.length;
      if(JSON.stringify(chunk.cut)!==JSON.stringify(page.read_cut)||chunk.domain!==page.domain||chunk.item_offset!==Number(item.offset)||chunk.offset!==offset||chunk.path.length!==0||bytes.length>65_536||!Number.isSafeInteger(chunk.readable_end)||end>chunk.readable_end||offset<chunk.readable_end&&!bytes.length||(chunk.next_offset??null)!==(end<chunk.readable_end?end:null))throw new Error('Native value range did not advance exactly.');
      setState({identity,page,item:item.identity,offset,next:chunk.next_offset??undefined,text:decoder.current.decode(bytes,{stream:chunk.next_offset!=null})});
    }catch(error){if(current())setState({identity,page,error:String(error).slice(0,512)});}
  };
  const chooseModel=async(item:CheckpointPageItem)=>{
    const page=loaded?.page,token=++generation.current,current=()=>token===generation.current&&currentIdentity.current===identity&&owned();
    if(!page||!onModel||!current())return;
    setState({identity,page,loading:true});
    try {await onModel(page,item,current);if(current())setState({identity,page});}
    catch(error){if(current())setState({identity,page,error:String(error).slice(0,512)});}
  };
  const growing=collections.filter(value=>value.next_offset!=null);
  if(!growing.length)return null;
  return <details><summary>{tx('common:activity.more-state')}</summary>
    <label>{tx('common:activity.collection')} <select value={selected??''} onChange={event=>{generation.current++;setSelected(event.target.value||undefined);}}><option value=""/>{growing.map(value=><option key={value.domain} value={value.domain}>{value.domain} ({value.total})</option>)}</select></label>
    {frontier&&<Button disabled={loaded?.loading||!owned()} onClick={()=>void loadPage(0)}>{tx('common:activity.first-page')}</Button>}
    {loaded?.page?.items.map(item=><div key={item.offset}><details><summary>{item.identity}</summary>{item.value!=null&&<pre>{JSON.stringify(item.value,null,2)}</pre>}{onAgent && loaded.page?.domain === 'agents' && !!item.value && typeof item.value === 'object' && 'agent_id' in item.value && typeof item.value.agent_id === 'string' && 'child_conversation_id' in item.value && 'activation_id' in item.value && <Button disabled={loaded.loading||!owned()} onClick={()=>onAgent(item.value as unknown as RuntimeClientAgent)}>{tx('common:subagents.inspect')}</Button>}{onModel&&(loaded.page?.domain==='model_catalog.models'||/^model_catalog\.models\.@[0-9]+\.profiles$/.test(loaded.page?.domain??''))&&<Button disabled={loaded.loading||!owned()} onClick={()=>void chooseModel(item)}>{tx('common:activity.select-model-defaults')}</Button>}<Button disabled={loaded.loading||!owned()} onClick={()=>void readValue(item)}>{tx('common:activity.read-value')}</Button></details></div>)}
    {loaded?.page?.next_offset!=null&&<Button disabled={loaded.loading||!owned()} onClick={()=>void loadPage(Number(loaded.page!.next_offset))}>{tx('common:activity.next-page')}</Button>}
    {loaded?.text&&<pre>{loaded.text}</pre>}
    {loaded?.next!=null&&<Button disabled={loaded.loading||!owned()} onClick={()=>{const item=loaded.page?.items.find(value=>value.identity===loaded.item);if(item)void readValue(item,loaded.next);}}>{tx('common:activity.next-content')}</Button>}
    {loaded?.error&&<p role="alert">{loaded.error}</p>}
  </details>;
}
