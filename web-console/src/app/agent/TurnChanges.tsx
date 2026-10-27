import { useContext, useEffect, useMemo, useState } from 'react';
import type { ChangesSummary } from '../../../../protocol/app-server/v61';
import { ChangedFiles } from '../../presentation/attachments/ChangedFiles';
import { useObservationSource, observationSourceIdentity } from './ObservationSource';
import { ChangesReviewContext, changesReviewOwner, type ChangesReviewRequest } from '../changes-review';
import { ChangeComparison } from '../components/ChangesReview';
import { useConversationPreferences } from '../conversation-preferences';

/** A completed native Attempt's actual changes, unavailable after its runtime retires. */
export function TurnChanges({attemptId,conversationId}: {attemptId:string;conversationId:string}) {
  const source=useObservationSource(), open=useContext(ChangesReviewContext);
  const [preferences]=useConversationPreferences();
  const identity=JSON.stringify([observationSourceIdentity(source),attemptId,conversationId]);
  const observation=source&&'target' in source ? source.client.getSnapshot().views[source.target.session_id]?.attachmentObservation : undefined;
  const reviewOwned=useMemo(()=>source&&'target' in source ? changesReviewOwner(source.client,source.target) : ()=>false,[identity,source?.client,observation]);
  const [state,setState]=useState<{identity:string;summary:ChangesSummary|null}>();
  const active=preferences.codingView && !!source?.owned() && source.cut()?.conversation_id===conversationId;
  useEffect(()=>{
    if(!source||!('target' in source)||source.agentId||!active)return;
    let live=true;
    const owned=()=>live&&source.owned();
    void source.client.request({method:'conversation/changes',params:{target:source.target,attempt_id:attemptId}},'workspace_changes',undefined,owned)
      .then(value=>{if(owned())setState({identity,summary:value.summary ?? null});},()=>{});
    return()=>{live=false;};
  },[identity,source?.client,source?.owned,active]);
  if(!source||!('target' in source)||source.agentId||!active||state?.identity!==identity||!state.summary?.files.length)return null;
  // Never use a current runtime's records for a different historical lineage.
  const cut=source.cut();
  if(cut && cut.conversation_id!==conversationId)return null;
  const request:ChangesReviewRequest={client:source.client,target:source.target,attemptId,summary:state.summary,index:0,owned:reviewOwned};
  return <ChangedFiles changes={state.summary} openReview={index=>open?.({...request,index})} preview={index=><ChangeComparison request={request} index={index} preview/>}/>;
}
