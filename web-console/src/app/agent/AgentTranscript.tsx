import type { RuntimeClientSnapshot } from '../../../../protocol/app-server/v9';
import { conversation, json } from '../../bindings/projection';
import { Button } from '../../presentation/primitives/Button';
import { AssistantMessage } from '../../presentation/agent/Message';
import { Feedback } from '../components/ConversationFeedback';
import { Content, Message } from './Message';
import { entryIdentity, HISTORY_LIMIT, type TranscriptCache } from '../../client/transcript';
import type { HistoryAction } from '../commands/native';
import css from '../../presentation/agent/Chat.module.css';
export function AgentTranscript({ snapshot, history, loadEarlier, latest, onHistorical, historicalDisabled, lineageSwitchSafe = false }: { snapshot: RuntimeClientSnapshot; history?: TranscriptCache; loadEarlier?: () => void; latest?: () => void; onHistorical?: (action: HistoryAction, messageId: string) => void; historicalDisabled?: boolean; lineageSwitchSafe?: boolean }) {
  const { messages, streaming } = conversation(snapshot);
  const entries = history?.page.entries ?? snapshot.transcript.entries ?? [];
  const durableIds = new Set(entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []));
  return <div className={css.column} aria-label="Canonical conversation">
    {history?.page.next_cursor != null && <Button disabled={history.loading || entries.length >= HISTORY_LIMIT} onClick={loadEarlier}>{history.loading ? 'Loading earlier…' : 'Load earlier'}</Button>}
    {entries.length >= HISTORY_LIMIT && <p>History window is full. <Button onClick={latest}>Return to latest</Button></p>}
    {history?.error && <p role="alert">{history.error}</p>}
    {!messages.length && !entries.length && <Feedback kind="empty" title="Ready for a task."><p>What would you like to work on?</p></Feedback>}
    {entries.filter(entry => entry.item.type !== 'message' || entry.item.message.role !== 'tool').map(entry => <div key={entryIdentity(entry)} data-chat-anchor-key={entryIdentity(entry)}>
      {entry.item.type === 'message' ? <Message message={entry.item.message} tools={(entry.tool_calls ?? []).map(tool => tool.state.type === 'settled' ? tool : snapshot.attempt?.foreground?.find(live => live.message_id === tool.message_id && live.block_index === tool.block_index && live.call_id === tool.call_id && live.tool_id === tool.tool_id) ?? tool)} /> : <details>
        <summary>{entry.item.type === 'publication_audit' ? 'Assistant recovery details' : 'Historical interaction details'}</summary>
        <pre>{json(entry.item)}</pre>
      </details>}
      {onHistorical && entry.item.type === 'message' && entry.item.message.role === 'user' && (!entry.item.message.kind || entry.item.message.kind === 'message') && <div className="row" aria-label="History actions">
        {(['fork', 'branch', 'retry'] as const).map(action => <Button size="sm" key={action} disabled={historicalDisabled || (action !== 'fork' && !lineageSwitchSafe)}
          onClick={() => { if (entry.item.type === 'message') onHistorical(action, entry.item.message.id); }}>{action === 'fork' ? 'Fork' : action === 'branch' ? 'Branch' : 'Retry / Regenerate'}</Button>)}
      </div>}
    </div>)}
    {messages.some(message => message.role === 'user' && message.kind && message.kind !== 'message' && !durableIds.has(message.id)) && <details><summary>Current context</summary>{messages.filter(message => message.role === 'user' && message.kind && message.kind !== 'message' && !durableIds.has(message.id)).map(message => <Message key={message.id} message={message} />)}</details>}
    {streaming && !durableIds.has(streaming.message_id) && <div data-chat-anchor-key={`message:${streaming.message_id}`}><AssistantMessage label="Streaming response"><Content blocks={streaming.blocks ?? []} markdown streaming tools={snapshot.attempt?.foreground?.filter(tool => tool.message_id === streaming.message_id)}/></AssistantMessage></div>}
  </div>;
}
