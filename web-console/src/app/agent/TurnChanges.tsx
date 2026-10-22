import { useContext, useEffect, useState } from 'react';
import type { ChangesSummary } from '../../../../protocol/app-server/v52';
import { ChangedFiles } from '../../presentation/attachments/ChangedFiles';
import { useObservationSource, observationSourceIdentity } from './ObservationSource';
import { ChangesReviewContext, type ChangesReviewRequest } from '../changes-review';
import { ChangeComparison } from '../components/ChangesReview';
import { useConversationPreferences } from '../conversation-preferences';

/** A completed native Attempt's actual changes, unavailable after its runtime retires. */
export function TurnChanges({attemptId,conversationId}: {attemptId:string;conversationId:string}) {
  const source=useObservationSource(), open=useContext(ChangesReviewContext);
  const [preferences]=useConversationPreferences();
  const identity=JSON.stringify([observationSourceIdentity(source),attemptId,conversationId]);
  const [state,setState]=useState<{identity:string;summary:ChangesSummary|null}>();
  const active=preferences.codingView && !!source?.owned() && source.cut()?.conversation_id===conversationId;
  useEffect(()=>{
    if(!source||!('target' in source)||source.agentId||!active)return;
    let live=true;
    const owned=()=>live&&source.owned();
    void source.client.request({method:'conversation/changes',params:{target:source.target,attempt_id:attemptId}},'workspace_changes',undefined,owned)
      .then(value=>{if(owned())setState({identity,summary:value.summary ?? null});},()=>{});
    return()=>{live=false;};
  },[identity,source?.client,active]);
  if(!source||!('target' in source)||source.agentId||!active||state?.identity!==identity||!state.summary?.files.length)return null;
  // Never use a current runtime's records for a different historical lineage.
  const cut=source.cut();
  if(cut && cut.conversation_id!==conversationId)return null;
  const request:ChangesReviewRequest={client:source.client,target:source.target,attemptId,summary:state.summary,index:0,owned:source.owned};
  return <ChangedFiles changes={state.summary} openReview={index=>open?.({...request,index})} preview={index=><ChangeComparison request={request} index={index} preview/>}/>;
}
