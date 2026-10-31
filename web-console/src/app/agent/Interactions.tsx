import { useTranslation } from '../../locale/react';
import { useEffect, useRef, useState } from 'react';
import type { AppServerClient } from '../../client/app-server';
import type { ClientView, SessionView } from '../../client/contracts';
import { interactionKey } from '../../client/contracts';
import { json } from '../../bindings/projection';
import { ApprovalTakeover } from '../../presentation/agent/ApprovalTakeover';
import { Questionnaire } from './Questionnaire';
import { Button } from '../../presentation/primitives/Button';
export function Interactions({ client, state, view }: {
  client: AppServerClient; state: ClientView; view: SessionView;
}) {
  const tx = useTranslation();
  const ownership=useRef({live:true,generation:0});
  const [failure, setFailure] = useState<{ key: string; message: string }>();
  const [loading,setLoading]=useState(false);
  const pending=client.pendingInteractions(view.id);
  const selected=pending===view.interactionPage?.rows?view.interactionPage:undefined;
  const cut=selected?.page.read_cut??view.checkpoint?.read_cut;
  // Unrelated publication cuts do not replace the immutable pending body.
  // Retain its original read cut while fencing the exact attachment and source.
  const identity=JSON.stringify([state.generation,state.authorityRevision,view.id,view.target,view.attachmentObservation,
    cut?.conversation_id,cut?.activation_id,cut?.domain_revisions.pending_interactions??cut?.revision,
    cut?.collection_cuts?.pending_primary,cut?.collection_cuts?.pending_children,selected?.offset??0,
    pending.map(({body,...header})=>header)]);
  useEffect(()=>{
    ownership.current={live:true,generation:ownership.current.generation+1};
    setFailure(undefined);setLoading(false);
    return()=>{ownership.current.live=false;ownership.current.generation++;};
  },[identity]);
  const run = (key: string, action: () => Promise<unknown>) => {
    const captured=ownership.current.generation;
    setFailure(undefined);
    void action().catch(cause=>{if(ownership.current.live&&ownership.current.generation===captured)setFailure({key,message:String(cause).slice(0,512)});});
  };
  const frontier=view.checkpoint?.collections.find(value=>value.domain==='pending_interactions');
  const next=selected?selected.page.next_offset:frontier?.next_offset;
  const page=(offset:number)=>{
    const captured=ownership.current.generation;
    const current=()=>ownership.current.live&&ownership.current.generation===captured;
    setLoading(true);setFailure(undefined);
    void client.interactionPage(view.id,offset).catch(error=>{if(current())setFailure({key:'page',message:String(error).slice(0,512)});}).finally(()=>{if(current())setLoading(false);});
  };
  const missingBody=pending.findIndex(item=>!client.interactionBody(view.id,item)&&!(item.kind==='approval'&&item.approval));
  useEffect(()=>{
    if(missingBody<0)return;
    let active=true;const current=()=>active&&ownership.current.live;
    const key=interactionKey(pending[missingBody].interaction);
    void client.loadInteractionBody(view.id,missingBody,current).catch(error=>{if(current())setFailure({key,message:String(error).slice(0,512)});});
    return()=>{active=false;};
  },[client,view.id,identity,missingBody]);
  return <>{pending.map((item,index) => {
    const key = interactionKey(item.interaction);
    const operation = state.interactionOperations[key]?.status;
    const disabled = !client.isAttachmentControlCurrent(view.id, view.attachmentObservation) || !!operation;
    const status = operation === 'uncertain' ? tx('interactions:copy.needs-verification-your-response-may-have-been-received')
      : operation === 'acknowledged' ? tx('interactions:copy.response-received-updating')
      : operation === 'in-flight' ? tx('interactions:copy.sending-response') : disabled ? tx('interactions:copy.reconnect-to-respond') : tx('interactions:copy.waiting-for-your-response');
    const body=client.interactionBody(view.id,item);
    if(!body&&item.kind==='approval'&&item.approval){
      const approval=item.approval;
      const response=(decision:'allow'|'deny')=>run(key,()=>client.answer(view.id,item.interaction,{type:'approval',decision:decision==='allow'?{type:'allow'}:{type:'deny',reason:'Denied by developer in rustX Web Console.'}}));
      return <div key={key} className="interaction">
        {failure?.key===key&&<p role="alert">{failure.message}</p>}
        <ApprovalTakeover title={tx('interactions:interactions.value-value',{p0:approval.tool_name,p1:approval.reason})}
          detail={<InteractionField client={client} view={view} index={index} identity={key} path={['request','kind','arguments']}/>} disabled={disabled} status={status} onAllow={()=>response('allow')} onDeny={()=>response('deny')}/>
        <Button disabled={disabled} onClick={()=>run(key,()=>client.answer(view.id,item.interaction))}>{tx('interactions:interactions.cancel-interaction')}</Button>
      </div>;
    }
    if(!body)return <section key={key} className="interaction">
      <p role="status">{tx('common:activity.loading')} · {item.kind}</p>
      {failure?.key===key&&<p role="alert">{failure.message}</p>}

      <Button disabled={disabled} onClick={()=>run(key,()=>client.answer(view.id,item.interaction))}>{tx('interactions:interactions.cancel-interaction')}</Button>
    </section>;
    const kind = body.request.kind;
    const response = (answer: Parameters<AppServerClient['answer']>[2]) => run(key, () => client.answer(view.id, item.interaction, answer));
    return <div key={key} className="interaction">
      {failure?.key === key && <p role="alert">{failure.message}</p>}
      {item.source.type === 'subagent' && <small className="inset muted">{tx('interactions:interactions.subagent-request')}</small>}
      {kind.type === 'approval' ? <ApprovalTakeover title={tx('interactions:interactions.value-value', { p0: kind.tool_name, p1: kind.reason })} detail={<pre>{json(kind.arguments)}</pre>}
        disabled={disabled} status={status} onAllow={() => response({ type: 'approval', decision: { type: 'allow' } })}
        onDeny={() => response({ type: 'approval', decision: { type: 'deny', reason: 'Denied by developer in rustX Web Console.' } })} />
        : kind.type === 'questionnaire' ? <Questionnaire key={key} questions={kind.questionnaire.questions} disabled={disabled}
          status={operation || disabled ? status : undefined} submitting={operation === 'in-flight'}
          onSubmit={value => response({ type: 'questionnaire', response: { type: 'submitted', value } })}
          onDecline={() => response({ type: 'questionnaire', response: { type: 'declined' } })} />
          // A Workflow Human Input: its actions are the options; dismissing it cancels.
          : <Questionnaire key={key} questions={kind.review.questionnaire.questions} disabled={disabled}
            form={{ eyebrow: `${kind.review.instance.block.definition.workflow_id} · ${kind.review.instance.node}`, title: tx('interactions:review.title') }}
            status={operation || disabled ? status : undefined} submitting={operation === 'in-flight'}
            onSubmit={submission => response({ type: 'review', response: { instance: kind.review.instance, subject_digest: kind.subject_digest, submission } })}
            onDecline={() => run(key, () => client.answer(view.id, item.interaction))} />}
      {/* A questionnaire's own dismiss action is its decline. */}
      {kind.type === 'approval' && <Button size="sm" disabled={disabled} onClick={() => run(key, () => client.answer(view.id, item.interaction))}>{tx('interactions:interactions.cancel-interaction')}</Button>}
    </div>;
  })}
  {failure?.key==='page'&&<p role="alert">{failure.message}</p>}
  {selected&&selected.offset>0&&<Button disabled={loading} onClick={()=>page(0)}>{tx('common:activity.first-interactions')}</Button>}
  {next!=null&&<Button disabled={loading||!client.isAttachmentControlCurrent(view.id,view.attachmentObservation)} onClick={()=>page(Number(next))}>{tx('common:activity.next-interactions')}</Button>}
  </>;
}
function InteractionField({client,view,index,identity,path}:{client:AppServerClient;view:SessionView;index:number;identity:string;path:string[]}) {
  const tx=useTranslation(),generation=useRef(0);
  const [state,setState]=useState<{text?:string;loading?:boolean;error?:string}>({});
  const key=JSON.stringify([view.target,view.checkpoint?.read_cut.domain_revisions.pending_interactions,view.interactionPage?.offset,index,identity,path]);
  useEffect(()=>{
    const token=++generation.current,current=()=>generation.current===token;
    const decoder=new TextDecoder('utf-8',{ignoreBOM:true,fatal:true});
    setState({loading:true});
    void(async()=>{
      const parts:string[]=[];let offset:number|undefined=0;
      do {const chunk=await client.interactionValueChunk(view.id,index,path,offset,current);if(!current())return;parts.push(decoder.decode(chunk.bytes,{stream:chunk.next!==undefined}));offset=chunk.next;}while(offset!==undefined);
      if(current())setState({text:parts.join('')});
    })().catch(error=>{if(current())setState({error:String(error).slice(0,512)});});
    return()=>{generation.current++;};
  },[key,client]);
  return <>{state.loading&&<p role="status">{tx('common:activity.loading')}</p>}{state.text&&<pre>{state.text}</pre>}{state.error&&<p role="alert">{state.error}</p>}</>;
}
