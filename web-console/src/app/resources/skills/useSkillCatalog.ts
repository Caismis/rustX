import {useEffect,useState} from 'react';
import type {SkillDescriptor,SourceScope,SourceSettings} from '../../../../../protocol/app-server/v78';
import type {AppServerClient} from '../../../client/app-server';

/** Native discovery and display information share a paginated catalog revision. */
export function useSkillCatalog(client:AppServerClient|undefined,source:SourceSettings,enabled:boolean,scope:SourceScope=source.target.kind,refresh=0) {
 const directory=scope==='workspace'&&source.target.kind==='workspace'?source.target.directory:undefined;
 const generation=client?.getSnapshot().generation;
 const [result,setResult]=useState<{source:SourceSettings;scope:SourceScope;directory?:string;generation:number;refresh:number;skills:SkillDescriptor[];error?:string}>();
 useEffect(()=>{
  if(!enabled||!client)return;
  let alive=true;
  const target=scope==='user'?{kind:'user' as const}:source.target;
  const generation=client.getSnapshot().generation;
  const current=()=>alive&&client.getSnapshot().generation===generation;
  void (async()=>{
   const skills:SkillDescriptor[]=[];
   let offset=0,revision:string|undefined;
   do {
    const {page}=await client.request({method:'skills/list',params:{target,offset,limit:32,expected_revision:revision}},'skills',undefined,current);
    if(!current())return;
    revision=page.revision;skills.push(...page.data);
    if(page.next_offset==null)break;
    offset=page.next_offset;
   } while(current());
   if(current())setResult({source,scope,directory,generation,refresh,skills});
  })().catch(error=>{if(current())setResult({source,scope,directory,generation,refresh,skills:[],error:String(error)});});
  return()=>{alive=false;};
 },[client,source,enabled,scope,directory,generation,refresh]);
 return enabled&&result&&result.scope===scope&&result.directory===directory&&result.generation===generation
  ? {...result,pending:result.source!==source||result.refresh!==refresh}:undefined;
}
