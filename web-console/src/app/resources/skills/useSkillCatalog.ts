import {useEffect,useState} from 'react';
import type {SkillDescriptor,SourceScope,SourceSettings} from '../../../../../protocol/app-server/v78';
import type {AppServerClient} from '../../../client/app-server';

/** Native discovery, including descriptions, is independently revisioned and
 * paginated. Neither the browser nor a Session scans or authors skill files. */
export function useSkillCatalog(client:AppServerClient|undefined,source:SourceSettings,enabled:boolean,scope:SourceScope=source.target.kind) {
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
 },[client,source,enabled,scope]);
 return result?.source===source&&result.scope===scope?result:undefined;
}
