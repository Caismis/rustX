import type { RuntimeClientSnapshot } from '../../../protocol/app-server/v4';
import type { AppServerClient, ClientView, SessionView } from '../client/app-server';
import { interactionKey } from '../client/app-server';
import { conversation, json } from '../bindings/projection';
import { SubagentCard, WorkflowCard, workflowKey } from './components/ActivityCards';
import { ToolArtifacts } from './components/Artifact';
import { MessageItem } from './components/MessageItem';
import { ToolRow } from './components/ToolRow';
import { ApprovalPanel } from './components/ApprovalPanel';
import { QuestionComposer } from './components/QuestionComposer';
import { Button } from '../presentation/primitives/Button';
import { Feedback } from '../presentation/primitives/Surface';
import { Content, Message } from './components/ChatMessage';
import { entryIdentity, HISTORY_LIMIT, type TranscriptCache } from '../client/transcript';
import { useState } from 'react';

export function Conversation({ snapshot, history, loadEarlier, latest }: { snapshot: RuntimeClientSnapshot; history?: TranscriptCache; loadEarlier?: () => void; latest?: () => void }) {
  const { messages, streaming } = conversation(snapshot);
  const entries = history?.page.entries ?? snapshot.transcript.entries ?? [];
  const durableIds = new Set(entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []));
  return <div className="messages" aria-label="Canonical conversation">
    {history?.page.next_cursor != null && <Button disabled={history.loading || entries.length >= HISTORY_LIMIT} onClick={loadEarlier}>{history.loading ? 'Loading earlier…' : 'Load earlier'}</Button>}
    {entries.length >= HISTORY_LIMIT && <p>History window is full. <Button onClick={latest}>Return to latest</Button></p>}
    {history?.error && <p role="alert">{history.error}</p>}
    {!messages.length && !entries.length && <Feedback kind="empty" title="Ready for a task."><p>This Session’s conversation is owned by rustX.</p></Feedback>}
    {entries.map(entry => <div key={entryIdentity(entry)} data-chat-anchor-key={entryIdentity(entry)}>
      {entry.item.type === 'message' ? <Message message={entry.item.message} /> : <details>
        <summary>{entry.item.type === 'publication_audit' ? 'Assistant publication / recovery' : `Historical interaction · ${entry.item.interaction_id}`}</summary>
        <pre>{json(entry.item)}</pre>
      </details>}
    </div>)}
    {messages.some(message => message.role === 'user' && message.kind && message.kind !== 'message' && !durableIds.has(message.id)) && <details><summary>Current context</summary>{messages.filter(message => message.role === 'user' && message.kind && message.kind !== 'message' && !durableIds.has(message.id)).map(message => <Message key={message.id} message={message} />)}</details>}
    {streaming && !durableIds.has(streaming.message_id) && <div data-chat-anchor-key={`message:${streaming.message_id}`}><MessageItem user={false} label={`Streaming · ${streaming.message_id}`}><Content blocks={streaming.blocks ?? []} markdown streaming /></MessageItem></div>}
  </div>;
}
export function RuntimeFacts({ snapshot }: { snapshot: RuntimeClientSnapshot }) {
  const tools = snapshot.attempt?.foreground ?? [];
  const children = snapshot.subagents ?? [];
  const background = snapshot.background ?? [];
  const workflows = snapshot.workflows.runs;
  if (!tools.length && !children.length && !workflows.length && !background.length && !snapshot.statuses?.length) return null;
  return <section className="runtime-facts" aria-label="Current activity">
    <small>Current activity</small>
    {tools.map(tool => <div key={tool.call_id} data-tool-call-id={tool.call_id}><ToolRow title={tool.name}
      summary={`${tool.state.type} · ${tool.call_id}`} running={tool.state.type === 'running'} input={tool.state.arguments}
      output={'result' in tool.state ? json(tool.state.result) : 'progress' in tool.state ? json(tool.state.progress) : undefined} />
      {'result' in tool.state && <ToolArtifacts result={tool.state.result} />}
    </div>)}
    {background.map(tool => <div key={tool.execution_id} data-execution-id={tool.execution_id}><ToolRow title={tool.tool_name} summary={`${tool.state} · ${tool.execution_id}`} output={tool.result ? json(tool.result) : undefined} running={tool.state === 'running'} />{tool.result && <ToolArtifacts result={tool.result} />}</div>)}
    {children.map(child => <SubagentCard key={child.subagent_id} child={child} />)}
    {workflows.map(workflow => <WorkflowCard key={workflowKey(workflow.id)} run={workflow} />)}
    {!!snapshot.statuses?.length && <details><summary>Agent Status ({snapshot.statuses.length})</summary><pre>{json(snapshot.statuses)}</pre></details>}
  </section>;
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
