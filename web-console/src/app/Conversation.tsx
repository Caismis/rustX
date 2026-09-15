import type { MessageBlock, RuntimeClientSnapshot, UserContentBlock, AssistantContentBlock, InFlightBlock } from '../../../protocol/app-server/v1';
import type { AppServerClient, ClientView, SessionView } from '../client/app-server';
import { interactionKey } from '../client/app-server';
import { conversation, json } from '../bindings/projection';
import { MessageItem } from './components/MessageItem';
import { ToolRow } from './components/ToolRow';
import { ApprovalPanel } from './components/ApprovalPanel';
import { QuestionComposer } from './components/QuestionComposer';
import { Button } from '../presentation/primitives/Button';
import { Feedback } from '../presentation/primitives/Surface';
import { MarkdownText } from '../presentation/markdown/MarkdownText';
import { useState } from 'react';

function Content({ blocks, markdown = false, streaming = false }: { markdown?: boolean; streaming?: boolean; blocks: (UserContentBlock | AssistantContentBlock | InFlightBlock)[] }) {
  return blocks.map((block, index) => {
    if (block.type === 'text') return markdown ? <MarkdownText key={index} text={block.text} streaming={streaming} /> : <span key={index}>{block.text}</span>;
    if (block.type === 'reasoning') return <details key={index}><summary>Reasoning</summary>{block.text}</details>;
    if (block.type === 'refusal') return <p key={index}>{block.text}</p>;
    if (block.type === 'tool_call') return <ToolRow key={index} title={block.name} summary="Tool call" input={typeof block.arguments === 'string' ? block.arguments : json(block.arguments)} />;
    return <details key={index}><summary>{block.type} reference</summary><pre>{json(block)}</pre></details>;
  });
}
function Message({ message }: { message: MessageBlock }) {
  return <MessageItem user={message.role === 'user'} label={`${message.role} · ${message.id}`}>
    {message.role === 'tool' ? <ToolRow title={message.tool_id} summary={message.result.status.type} output={json(message.result)} /> : <Content blocks={message.content} markdown={message.role === 'assistant'} />}
  </MessageItem>;
}
export function Conversation({ snapshot }: { snapshot: RuntimeClientSnapshot }) {
  const { messages, streaming } = conversation(snapshot);
  return <div className="messages" aria-label="Canonical conversation">
    {!messages.length && <Feedback kind="empty" title="Ready for a task."><p>This Session’s conversation is owned by rustX.</p></Feedback>}
    {messages.map(message => <Message key={message.id} message={message} />)}
    {streaming && <MessageItem user={false} label={`Streaming · ${streaming.message_id}`}><Content blocks={streaming.blocks ?? []} markdown streaming /></MessageItem>}
  </div>;
}
export function RuntimeFacts({ snapshot }: { snapshot: RuntimeClientSnapshot }) {
  return <div className="runtime-facts">
    {snapshot.attempt?.foreground?.map(tool => <ToolRow key={tool.call_id} title={tool.name}
      summary={`${tool.state.type} · ${tool.call_id}`} running={tool.state.type === 'running'} input={tool.state.arguments}
      output={'result' in tool.state ? json(tool.state.result) : 'progress' in tool.state ? json(tool.state.progress) : undefined} />)}
    {([
      ['Subagents', snapshot.subagents], ['Background', snapshot.background], ['Workflows', snapshot.workflows],
      ['Todo', snapshot.todos], ['Goal', snapshot.goal], ['Inbound', snapshot.inbound], ['Agent Status', snapshot.statuses],
    ] as const).map(([label, value]) => value != null && <details key={label}><summary>{label}{Array.isArray(value) ? ` (${value.length})` : ''}</summary><pre>{json(value)}</pre></details>)}
  </div>;
}
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
      {kind.type === 'approval' ? <ApprovalPanel title={`${kind.tool_name}: ${kind.reason}`} detail={<pre>{json(kind.arguments)}</pre>}
        disabled={disabled} status={status} onAllow={() => response({ type: 'approval', decision: { type: 'allow' } })}
        onDeny={() => response({ type: 'approval', decision: { type: 'deny', reason: 'Denied by developer in rustX Web Console.' } })} />
        : kind.type === 'questionnaire' ? <QuestionComposer key={key} questions={kind.questionnaire.questions} disabled={disabled} status={`${kind.requester.tool_name} · ${status}`}
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
