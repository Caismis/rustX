import {useEffect,useState} from 'react';
import type {SkillDescriptor,SourceScope,SourceSettings,SourceTarget,WorkspaceCatalogAuthority} from '../../../../../protocol/app-server/v81';
import type {AppServerClient} from '../../../client/app-server';
import {RpcFailure} from '../../../client/errors';

/** Native discovery and display information share a paginated catalog revision. */
export function useSkillCatalog(client:AppServerClient|undefined,source:SourceSettings|undefined,enabled:boolean,scope:SourceScope,refresh=0,executionSession?:string,target:SourceTarget|undefined=source?.target) {
 const directory=scope==='workspace'&&target?.kind==='workspace'?target.directory:undefined;
 const session=scope==='workspace'?executionSession:undefined;
 const generation=client?.getSnapshot().generation;
 const [result,setResult]=useState<{source?:SourceSettings;scope:SourceScope;directory?:string;session?:string;generation:number;refresh:number;skills:SkillDescriptor[];authority?:WorkspaceCatalogAuthority;clientAuthority?:boolean;error?:string}>();
 useEffect(()=>{
  if(!enabled||!client||(scope==='workspace'&&!directory))return;
  let alive=true;
  const target:SourceTarget=scope==='user'?{kind:'user'}:{kind:'workspace',directory:directory!};
  const generation=client.getSnapshot().generation;
  const current=()=>alive&&client.getSnapshot().generation===generation;
  let authority:WorkspaceCatalogAuthority|undefined;
  void (async()=>{
   const skills:SkillDescriptor[]=[];
   let offset=0,revision:string|undefined;
   do {
    const {page,authority:pageAuthority}=await client.request({method:'skills/list',params:{target,execution_session:session,offset,limit:32,expected_revision:revision}},'skills',undefined,current);
    if(!current())return;
    if(authority&&authority!==pageAuthority)throw new Error('Workspace catalog authority changed during pagination');
    authority=pageAuthority;
    revision=page.revision;skills.push(...page.data);
    if(page.next_offset==null)break;
    offset=page.next_offset;
   } while(current());
   if(current())setResult({source,scope,directory,session,generation,refresh,skills,authority});
  })().catch(error=>{
    if (!current()) return;
    setResult(previous => {
      const sameScope = previous?.scope === scope && previous.directory === directory
        && previous.session === session && previous.generation === generation;
      const clientAuthority = authority === 'client'
        || (error instanceof RpcFailure && error.error.data?.kind === 'execution_unavailable')
        || (sameScope && (previous.authority === 'client' || previous.clientAuthority === true));
      return {source,scope,directory,session,generation,refresh,skills:[],clientAuthority,error:String(error)};
    });
  });
  return()=>{alive=false;};
 },[client,source,enabled,scope,directory,session,generation,refresh]);
 return enabled&&result&&result.scope===scope&&result.directory===directory&&result.session===session&&result.generation===generation
  ? {...result,pending:result.source!==source||result.refresh!==refresh}:undefined;
}
