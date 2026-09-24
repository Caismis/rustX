import type { RuntimeClientSnapshot, CompletedResponseView, RuntimeClientTranscriptEntry } from '../../../../protocol/app-server/v20';
import { conversation, json } from '../../bindings/projection';
import { agentStatusPlacement, isAgentStatusContext, statusesAt } from '../../bindings/agent-status';
import { Button } from '../../presentation/primitives/Button';
import { AssistantMessage } from '../../presentation/agent/Message';
import { Feedback } from '../components/ConversationFeedback';
import { AgentStatusAnnotation } from './AgentStatus';
import { Content, Message } from './Message';
import { entryIdentity, HISTORY_LIMIT, type TranscriptCache } from '../../client/transcript';
import type { HistoryAction } from '../commands/native';
import { CopyMessage, MessageTime, ResponseTail } from './ResponseTail';
import tailCss from './ResponseTail.module.css';
import css from '../../presentation/agent/Chat.module.css';
/** Standalone Tool-result bodies are suppressed: the native call projection
 * already renders their content beside the call. Suppressing a body does not
 * erase the entry's authoritative transcript position, so a `PostToolBatch`
 * annotation anchored there still renders in place — as an annotation-only slot. */
const hasBody = (entry: RuntimeClientTranscriptEntry) => entry.item.type !== 'message' || entry.item.message.role !== 'tool';
export function AgentTranscript({ snapshot, history, loadEarlier, latest, onHistorical, historicalDisabled, lineageSwitchSafe = false }: { snapshot: RuntimeClientSnapshot; history?: TranscriptCache; loadEarlier?: () => void; latest?: () => void; onHistorical?: (action: HistoryAction, response: CompletedResponseView) => void; historicalDisabled?: boolean; lineageSwitchSafe?: boolean }) {
  const { messages, streaming } = conversation(snapshot);
  const entries = history?.page.entries ?? snapshot.transcript.entries ?? [];
  const latestResponse = entries.filter(entry => entry.completed_response).at(-1);
  const latestUser = entries.filter(entry => entry.item.type === 'message' && entry.item.message.role === 'user').at(-1);
  const durableIds = new Set(entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []));
  // Placement is a property of the authoritative status window and the runtime
  // facts each composition carries, never of what this page happens to hold: an
  // anchor outside the loaded transcript simply draws nothing until it is paged in.
  const placement = agentStatusPlacement(snapshot.statuses);
  // Agent Status has its own anchored annotation, so its canonical Context message
  // must not reappear here as ordinary chat or as purported current context.
  const currentContext = messages.filter(message => message.role === 'user' && message.kind && message.kind !== 'message' && !durableIds.has(message.id) && !isAgentStatusContext(message));
  return <div className={css.column} aria-label="Canonical conversation">
    {history?.page.next_cursor != null && <Button disabled={history.loading || entries.length >= HISTORY_LIMIT} onClick={loadEarlier}>{history.loading ? 'Loading earlier…' : 'Load earlier'}</Button>}
    {entries.length >= HISTORY_LIMIT && <p>History window is full. <Button onClick={latest}>Return to latest</Button></p>}
    {history?.error && <p role="alert">{history.error}</p>}
    {!messages.length && !entries.length && <Feedback kind="empty" title="Ready for a task."><p>What would you like to work on?</p></Feedback>}
    {entries.map(entry => {
      const statuses = statusesAt(placement, { messageId: entry.item.type === 'message' ? entry.item.message.id : undefined, cursor: entry.cursor });
      const body = hasBody(entry);
      if (!body && !statuses.length) return null;
      return <div key={entryIdentity(entry)} data-chat-anchor-key={entryIdentity(entry)} data-response-reveal={entry === latestResponse || entry === latestUser ? 'always' : 'hover'}>
        {body && (entry.item.type === 'message' ? <Message message={entry.item.message} actions={entry.item.type === 'message' && entry.item.message.role === 'user' && (!entry.item.message.kind || entry.item.message.kind === 'message') && <div className={tailCss.actions} aria-label="Message actions"><MessageTime time={entry.item.message.timestamp}/><CopyMessage text={entry.item.message.content.flatMap(block => block.type === 'text' ? [block.text] : []).join('')}/></div>} tools={(entry.tool_calls ?? []).map(tool => tool.state.type === 'settled' ? tool : snapshot.attempt?.foreground?.find(live => live.message_id === tool.message_id && live.block_index === tool.block_index && live.call_id === tool.call_id && live.tool_id === tool.tool_id) ?? tool)} /> : <details>
          <summary>{entry.item.type === 'publication_audit' ? 'Assistant recovery details' : 'Historical interaction details'}</summary>
          <pre>{json(entry.item)}</pre>
        </details>)}

        {body && entry.item.type === 'message' && entry.item.message.role === 'assistant' && entry.completed_response && <ResponseTail
          text={entry.item.message.content.flatMap(block => block.type === 'text' || block.type === 'refusal' ? [block.text] : []).join('')}
          response={entry.completed_response} onHistorical={onHistorical} disabled={historicalDisabled} lineageSwitchSafe={lineageSwitchSafe}/>}

        {statuses.map(status => <AgentStatusAnnotation key={status.status_message_id} status={status} />)}
      </div>;
    })}
    {!!currentContext.length && <details><summary>Current context</summary>{currentContext.map(message => <Message key={message.id} message={message} />)}</details>}
    {streaming && !durableIds.has(streaming.message_id) && <div data-chat-anchor-key={`message:${streaming.message_id}`}><AssistantMessage label="Streaming response"><Content blocks={streaming.blocks ?? []} markdown streaming tools={snapshot.attempt?.foreground?.filter(tool => tool.message_id === streaming.message_id)}/></AssistantMessage></div>}
  </div>;
}
