import {useEffect,useState} from 'react';
import type {SkillDescriptor,SourceSettings} from '../../../../../protocol/app-server/v77';
import type {AppServerClient} from '../../../client/app-server';

/** Native discovery, including descriptions, is independently revisioned and
 * paginated. Neither the browser nor a Session scans or authors skill files. */
export function useSkillCatalog(client:AppServerClient,source:SourceSettings,enabled:boolean) {
 const [result,setResult]=useState<{source:SourceSettings;skills:SkillDescriptor[];error?:string}>();
 useEffect(()=>{
  if(!enabled)return;
  let alive=true;
  const generation=client.getSnapshot().generation;
  const current=()=>alive&&client.getSnapshot().generation===generation;
  void (async()=>{
   const skills:SkillDescriptor[]=[];
   let offset=0,revision:string|undefined;
   do {
    const {page}=await client.request({method:'skills/list',params:{target:source.target,offset,limit:32,expected_revision:revision}},'skills',undefined,current);
    if(!current())return;
    revision=page.revision;skills.push(...page.data);
    if(page.next_offset==null)break;
    offset=page.next_offset;
   } while(current());
   if(current())setResult({source,skills});
  })().catch(error=>{if(current())setResult({source,skills:[],error:String(error)});});
  return()=>{alive=false;};
 },[client,source,enabled]);
 return result?.source===source?result:undefined;
}
