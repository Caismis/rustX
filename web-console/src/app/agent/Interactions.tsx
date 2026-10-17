import { useTranslation } from '../../locale/react';
import { useEffect, useRef, useState } from 'react';
import type { AppServerClient, ClientView, SessionView } from '../../client/app-server';
import { interactionKey } from '../../client/app-server';
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
      <p role="status">{tx('common:activity.content-deferred')} · {item.kind}</p>
      {failure?.key===key&&<p role="alert">{failure.message}</p>}
      <Button disabled={loading} onClick={()=>{
        const captured=ownership.current.generation;
        setLoading(true);
        run(key,()=>client.loadInteractionBody(view.id,index,()=>ownership.current.live&&ownership.current.generation===captured).finally(()=>{if(ownership.current.live&&ownership.current.generation===captured)setLoading(false);}));
      }}>{tx('common:activity.load-content',{bytes:0})}</Button>
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
          : <Review key={key} title={status} detail={json({ subject: kind.review.subject, context: kind.review.context })} disabled={disabled}
            accept={() => response({ type: 'review', response: { instance: kind.review.instance, subject_digest: kind.subject_digest, decision: { type: 'accepted' } } })}
            reject={feedback => response({ type: 'review', response: { instance: kind.review.instance, subject_digest: kind.subject_digest, decision: { type: 'rejected', feedback } } })} />}
      {/* A questionnaire's own dismiss action is its decline. */}
      {kind.type !== 'questionnaire' && <Button size="sm" disabled={disabled} onClick={() => run(key, () => client.answer(view.id, item.interaction))}>{tx('interactions:interactions.cancel-interaction')}</Button>}
    </div>;
  })}
  {failure?.key==='page'&&<p role="alert">{failure.message}</p>}
  {selected&&selected.offset>0&&<Button disabled={loading} onClick={()=>page(0)}>{tx('common:activity.first-interactions')}</Button>}
  {next!=null&&<Button disabled={loading||!client.isAttachmentControlCurrent(view.id,view.attachmentObservation)} onClick={()=>page(Number(next))}>{tx('common:activity.next-interactions')}</Button>}
  </>;
}
function Review({ title, detail, disabled, accept, reject }: {
  title: string; detail: string; disabled: boolean; accept: () => void; reject: (feedback: string) => void;
}) {
  const tx = useTranslation();
  const [feedback, setFeedback] = useState('');
  return <section className="review"><h3>{tx('interactions:interactions.review')}{' '}{title}</h3><pre>{detail}</pre>
    <textarea aria-label={tx('interactions:interactions.review-feedback')} value={feedback} disabled={disabled} onChange={event => setFeedback(event.target.value)} />
    <div className="row"><Button disabled={disabled || !feedback.trim()} onClick={() => reject(feedback)}>{tx('interactions:interactions.reject-with-feedback')}</Button><Button disabled={disabled} variant="primary" onClick={accept}>{tx('interactions:interactions.accept-review')}</Button></div>
  </section>;
}

function InteractionField({client,view,index,identity,path}:{client:AppServerClient;view:SessionView;index:number;identity:string;path:string[]}) {
  const tx=useTranslation(),generation=useRef(0),decoder=useRef(new TextDecoder('utf-8',{ignoreBOM:true}));
  const [state,setState]=useState<{text?:string;next?:number;loading?:boolean;error?:string}>({});
  const key=JSON.stringify([view.target,view.checkpoint?.read_cut.domain_revisions.pending_interactions,view.interactionPage?.offset,index,identity,path]);
  useEffect(()=>{generation.current++;setState({});decoder.current=new TextDecoder('utf-8',{ignoreBOM:true});return()=>{generation.current++;};},[key]);
  const load=async(offset=0)=>{
    const captured=++generation.current,current=()=>generation.current===captured;
    if(offset===0)decoder.current=new TextDecoder('utf-8',{ignoreBOM:true});
    setState({loading:true});
    try{
      const chunk=await client.interactionValueChunk(view.id,index,path,offset,current);
      if(current())setState({text:decoder.current.decode(chunk.bytes,{stream:chunk.next!==undefined}),next:chunk.next});
    }catch(error){if(current())setState({error:String(error).slice(0,512)});}
  };
  return <section aria-label={tx('common:activity.content-deferred')}>
    {!state.text&&<p role="status">{tx('common:activity.content-deferred')}</p>}
    <Button disabled={state.loading} onClick={()=>void load(state.next??0)}>{tx(state.next!==undefined?'common:activity.next-content':'common:activity.load-content',{bytes:0})}</Button>
    {state.text&&<pre>{state.text}</pre>}{state.error&&<p role="alert">{state.error}</p>}
  </section>;
}
