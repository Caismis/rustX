import type { RuntimeClientSnapshot, CompletedResponseView } from '../../../../protocol/app-server/v9';
import { conversation, json } from '../../bindings/projection';
import { Button } from '../../presentation/primitives/Button';
import { AssistantMessage } from '../../presentation/agent/Message';
import { Feedback } from '../components/ConversationFeedback';
import { Content, Message } from './Message';
import { entryIdentity, HISTORY_LIMIT, type TranscriptCache } from '../../client/transcript';
import type { HistoryAction } from '../commands/native';
import { CopyMessage, MessageTime, ResponseTail } from './ResponseTail';
import tailCss from './ResponseTail.module.css';
import css from '../../presentation/agent/Chat.module.css';
export function AgentTranscript({ snapshot, history, loadEarlier, latest, onHistorical, historicalDisabled, lineageSwitchSafe = false }: { snapshot: RuntimeClientSnapshot; history?: TranscriptCache; loadEarlier?: () => void; latest?: () => void; onHistorical?: (action: HistoryAction, response: CompletedResponseView) => void; historicalDisabled?: boolean; lineageSwitchSafe?: boolean }) {
  const { messages, streaming } = conversation(snapshot);
  const entries = history?.page.entries ?? snapshot.transcript.entries ?? [];
  const latestResponse = entries.filter(entry => entry.completed_response).at(-1);
  const latestUser = entries.filter(entry => entry.item.type === 'message' && entry.item.message.role === 'user').at(-1);
  const durableIds = new Set(entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []));
  return <div className={css.column} aria-label="Canonical conversation">
    {history?.page.next_cursor != null && <Button disabled={history.loading || entries.length >= HISTORY_LIMIT} onClick={loadEarlier}>{history.loading ? 'Loading earlier…' : 'Load earlier'}</Button>}
    {entries.length >= HISTORY_LIMIT && <p>History window is full. <Button onClick={latest}>Return to latest</Button></p>}
    {history?.error && <p role="alert">{history.error}</p>}
    {!messages.length && !entries.length && <Feedback kind="empty" title="Ready for a task."><p>What would you like to work on?</p></Feedback>}
    {entries.filter(entry => entry.item.type !== 'message' || entry.item.message.role !== 'tool').map(entry => <div key={entryIdentity(entry)} data-chat-anchor-key={entryIdentity(entry)} data-response-reveal={entry === latestResponse || entry === latestUser ? 'always' : 'hover'}>
      {entry.item.type === 'message' ? <Message message={entry.item.message} actions={entry.item.type === 'message' && entry.item.message.role === 'user' && (!entry.item.message.kind || entry.item.message.kind === 'message') && <div className={tailCss.actions} aria-label="Message actions"><MessageTime time={entry.item.message.timestamp}/><CopyMessage text={entry.item.message.content.flatMap(block => block.type === 'text' ? [block.text] : []).join('')}/></div>} tools={(entry.tool_calls ?? []).map(tool => tool.state.type === 'settled' ? tool : snapshot.attempt?.foreground?.find(live => live.message_id === tool.message_id && live.block_index === tool.block_index && live.call_id === tool.call_id && live.tool_id === tool.tool_id) ?? tool)} /> : <details>
        <summary>{entry.item.type === 'publication_audit' ? 'Assistant recovery details' : 'Historical interaction details'}</summary>
        <pre>{json(entry.item)}</pre>
      </details>}

      {entry.item.type === 'message' && entry.item.message.role === 'assistant' && entry.completed_response && <ResponseTail
        text={entry.item.message.content.flatMap(block => block.type === 'text' || block.type === 'refusal' ? [block.text] : []).join('')}
        response={entry.completed_response} onHistorical={onHistorical} disabled={historicalDisabled} lineageSwitchSafe={lineageSwitchSafe}/>}

    </div>)}
    {messages.some(message => message.role === 'user' && message.kind && message.kind !== 'message' && !durableIds.has(message.id)) && <details><summary>Current context</summary>{messages.filter(message => message.role === 'user' && message.kind && message.kind !== 'message' && !durableIds.has(message.id)).map(message => <Message key={message.id} message={message} />)}</details>}
    {streaming && !durableIds.has(streaming.message_id) && <div data-chat-anchor-key={`message:${streaming.message_id}`}><AssistantMessage label="Streaming response"><Content blocks={streaming.blocks ?? []} markdown streaming tools={snapshot.attempt?.foreground?.filter(tool => tool.message_id === streaming.message_id)}/></AssistantMessage></div>}
  </div>;
}
