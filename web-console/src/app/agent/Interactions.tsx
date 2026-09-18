import { useState } from 'react';
import type { AppServerClient, ClientView, SessionView } from '../../client/app-server';
import { interactionKey } from '../../client/app-server';
import { json } from '../../bindings/projection';
import { ApprovalTakeover } from '../../presentation/agent/ApprovalTakeover';
import { Questionnaire } from './Questionnaire';
import { Button } from '../../presentation/primitives/Button';
export function Interactions({ client, state, view, run }: {
  client: AppServerClient; state: ClientView; view: SessionView; run: (action: () => Promise<unknown>) => void;
}) {
  return view.snapshot?.pending_interactions?.map(item => {
    const key = interactionKey(item.interaction);
    const operation = state.interactionOperations[key]?.status;
    const disabled = view.attachmentIntent !== 'wanted' || view.attachment !== 'attached' || !!operation;
    const status = operation === 'uncertain' ? 'Outcome uncertain — waiting for authoritative resolution'
      : operation === 'acknowledged' ? 'Settlement acknowledged — refreshing'
      : operation === 'in-flight' ? 'Awaiting server acknowledgement' : view.attachmentIntent !== 'wanted' ? 'Pending — attachment released by this client' : view.attachment !== 'attached' ? 'Pending — stale until reattached' : 'Pending interaction';
    const kind = item.request.kind;
    const response = (answer: Parameters<AppServerClient['answer']>[2]) => run(() => client.answer(view.id, item.interaction, answer));
    return <div key={key} className="interaction">
      <small className="inset muted">{item.source.type === 'subagent' ? `Subagent ${item.source.subagent_id} · ` : ''}{item.interaction.conversation_id} / {item.interaction.interaction_id}</small>
      {kind.type === 'approval' ? <ApprovalTakeover title={`${kind.tool_name}: ${kind.reason}`} detail={<pre>{json(kind.arguments)}</pre>}
        disabled={disabled} status={status} onAllow={() => response({ type: 'approval', decision: { type: 'allow' } })}
        onDeny={() => response({ type: 'approval', decision: { type: 'deny', reason: 'Denied by developer in rustX Web Console.' } })} />
        : kind.type === 'questionnaire' ? <Questionnaire key={key} questions={kind.questionnaire.questions} disabled={disabled} status={`${kind.requester.tool_name} · ${status}`}
          onSubmit={value => response({ type: 'questionnaire', response: { type: 'submitted', value } })}
          onDecline={() => response({ type: 'questionnaire', response: { type: 'declined' } })} />
          : <Review key={key} title={status} detail={json(kind.review)} disabled={disabled}
            accept={() => response({ type: 'review', response: { instance: kind.review.instance, subject_digest: kind.subject_digest, decision: { type: 'accepted' } } })}
            reject={feedback => response({ type: 'review', response: { instance: kind.review.instance, subject_digest: kind.subject_digest, decision: { type: 'rejected', feedback } } })} />}
      <Button size="sm" disabled={disabled} onClick={() => run(() => client.answer(view.id, item.interaction))}>Cancel interaction</Button>
    </div>;
  });
}
function Review({ title, detail, disabled, accept, reject }: {
  title: string; detail: string; disabled: boolean; accept: () => void; reject: (feedback: string) => void;
}) {
  const [feedback, setFeedback] = useState('');
  return <section className="review"><h3>Review · {title}</h3><pre>{detail}</pre>
    <textarea aria-label="Review feedback" value={feedback} disabled={disabled} onChange={event => setFeedback(event.target.value)} />
    <div className="row"><Button disabled={disabled || !feedback.trim()} onClick={() => reject(feedback)}>Reject with feedback</Button><Button disabled={disabled} variant="primary" onClick={accept}>Accept review</Button></div>
  </section>;
}
