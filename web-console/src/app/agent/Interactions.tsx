import { useTranslation } from '../../locale/react';
import { useState } from 'react';
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
  const [failure, setFailure] = useState<{ key: string; message: string }>();
  const run = (key: string, action: () => Promise<unknown>) => { setFailure(undefined); void action().catch(cause => setFailure({ key, message: String(cause) })); };
  return view.snapshot?.pending_interactions?.map(item => {
    const key = interactionKey(item.interaction);
    const operation = state.interactionOperations[key]?.status;
    const disabled = view.attachmentIntent !== 'wanted' || view.attachment !== 'attached' || !!operation;
    const status = operation === 'uncertain' ? tx('interactions:copy.needs-verification-your-response-may-have-been-received')
      : operation === 'acknowledged' ? tx('interactions:copy.response-received-updating')
      : operation === 'in-flight' ? tx('interactions:copy.sending-response') : disabled ? tx('interactions:copy.reconnect-to-respond') : tx('interactions:copy.waiting-for-your-response');
    const kind = item.request.kind;
    const response = (answer: Parameters<AppServerClient['answer']>[2]) => run(key, () => client.answer(view.id, item.interaction, answer));
    return <div key={key} className="interaction">
      {failure?.key === key && <p role="alert">{failure.message}</p>}
      {item.source.type === 'subagent' && <small className="inset muted">{tx('interactions:interactions.subagent-request')}</small>}
      {kind.type === 'approval' ? <ApprovalTakeover title={tx('interactions:interactions.value-value', { p0: kind.tool_name, p1: kind.reason })} detail={<pre>{json(kind.arguments)}</pre>}
        disabled={disabled} status={status} onAllow={() => response({ type: 'approval', decision: { type: 'allow' } })}
        onDeny={() => response({ type: 'approval', decision: { type: 'deny', reason: 'Denied by developer in rustX Web Console.' } })} />
        : kind.type === 'questionnaire' ? <Questionnaire key={key} questions={kind.questionnaire.questions} disabled={disabled} status={`${kind.requester.tool_name} · ${status}`}
          onSubmit={value => response({ type: 'questionnaire', response: { type: 'submitted', value } })}
          onDecline={() => response({ type: 'questionnaire', response: { type: 'declined' } })} />
          : <Review key={key} title={status} detail={json({ subject: kind.review.subject, context: kind.review.context })} disabled={disabled}
            accept={() => response({ type: 'review', response: { instance: kind.review.instance, subject_digest: kind.subject_digest, decision: { type: 'accepted' } } })}
            reject={feedback => response({ type: 'review', response: { instance: kind.review.instance, subject_digest: kind.subject_digest, decision: { type: 'rejected', feedback } } })} />}
      <Button size="sm" disabled={disabled} onClick={() => run(key, () => client.answer(view.id, item.interaction))}>{tx('interactions:interactions.cancel-interaction')}</Button>
    </div>;
  });
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
