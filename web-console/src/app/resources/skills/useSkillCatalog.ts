import {useEffect,useState} from 'react';
import type {SkillDescriptor,SourceScope,SourceSettings} from '../../../../../protocol/app-server/v77';
import type {AppServerClient} from '../../../client/app-server';

/** Native discovery and display information share a paginated catalog revision. */
export function useSkillCatalog(client:AppServerClient|undefined,source:SourceSettings,enabled:boolean,scope:SourceScope=source.target.kind,refresh=0) {
 const [result,setResult]=useState<{source:SourceSettings;scope:SourceScope;skills:SkillDescriptor[];error?:string}>();
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
   if(current())setResult({source,scope,skills});
  })().catch(error=>{if(current())setResult({source,scope,skills:[],error:String(error)});});
  return()=>{alive=false;};
 },[client,source,enabled,scope,refresh]);
 return result?.source===source&&result.scope===scope?result:undefined;
}
