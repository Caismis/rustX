import { Fragment, useState } from 'react';
import { turnPresentation } from '../../bindings/turn-presentation';
import { turnProcesses } from '../../bindings/turn-process';
import { TurnProcess } from '../../presentation/agent/TurnProcess';
import type { RuntimeClientSnapshot, CompletedResponseView, RuntimeClientTranscriptEntry } from '../../../../protocol/app-server/v25';
import { conversation, json } from '../../bindings/projection';
import { agentStatusPlacement, isAgentStatusContext, statusesAt } from '../../bindings/agent-status';
import { Button } from '../../presentation/primitives/Button';
import { AssistantMessage } from '../../presentation/agent/Message';
import { Feedback } from '../components/ConversationFeedback';
import { AgentStatusAnnotation } from './AgentStatus';
import { Content, Message } from './Message';
import { entryIdentity, HISTORY_LIMIT, type TranscriptCache } from '../../client/transcript';
import type { HistoryAction } from '../commands/native';
import { CopyMessage, MessageTime, TurnTail } from './TurnTail';
import tailCss from './TurnTail.module.css';
import css from '../../presentation/agent/Chat.module.css';
/** Standalone Tool-result bodies are suppressed: the native call projection
 * already renders their content beside the call. Suppressing a body does not
 * erase the entry's authoritative transcript position, so a `PostToolBatch`
 * annotation anchored there still renders in place — as an annotation-only slot. */
const hasBody = (entry: RuntimeClientTranscriptEntry) => entry.item.type !== 'message' || entry.item.message.role !== 'tool';
export function AgentTranscript({ snapshot, history, loadEarlier, latest, onHistorical, historicalDisabled, lineageSwitchSafe = false }: { snapshot: RuntimeClientSnapshot; history?: TranscriptCache; loadEarlier?: () => void; latest?: () => void; onHistorical?: (action: HistoryAction, response: CompletedResponseView) => void; historicalDisabled?: boolean; lineageSwitchSafe?: boolean }) {
  const { messages, streaming } = conversation(snapshot);
  const entries = history?.page.entries ?? snapshot.transcript.entries ?? [];
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());

  const latestResponse = entries.filter(entry => entry.completed_response).at(-1);
  const latestUser = entries.filter(entry => entry.item.type === 'message' && entry.item.message.role === 'user').at(-1);
  const durableIds = new Set(entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []));
  // Placement is a property of the authoritative status window and the runtime
  // facts each composition carries, never of what this page happens to hold: an
  // anchor outside the loaded transcript simply draws nothing until it is paged in.
  const placement = agentStatusPlacement(snapshot.statuses);
  const process = turnProcesses(entries, placement, snapshot.conversation_id);
  const disclosure = (key: string) => {
    const group = process.groups.get(key)!;
    const response = entries.find(entry => entry.turn_process && JSON.stringify([entry.turn_process.conversation_id, entry.turn_process.attempt_id]) === key && entry.completed_response)?.completed_response;
    return <TurnProcess durationMs={response?.timing?.total_duration_ms ?? undefined} id={key} open={expanded.has(key)} tools={group.tools} messages={group.messages} toggle={() => setExpanded(previous => {
      const next = new Set(previous); if (next.has(key)) next.delete(key); else next.add(key); return next;
    })}/>;
  };
  // Agent Status has its own anchored annotation, so its canonical Context message
  // must not reappear here as ordinary chat or as purported current context.
  const currentContext = messages.filter(message => message.role === 'user' && message.kind && message.kind !== 'message' && !durableIds.has(message.id) && !isAgentStatusContext(message));
  return <div className={css.column} aria-label="Canonical conversation">
    {history?.page.next_cursor != null && <Button disabled={history.loading || entries.length >= HISTORY_LIMIT} onClick={loadEarlier}>{history.loading ? 'Loading earlier…' : 'Load earlier'}</Button>}
    {entries.length >= HISTORY_LIMIT && <p>History window is full. <Button onClick={latest}>Return to latest</Button></p>}
    {history?.error && <p role="alert">{history.error}</p>}
    {!messages.length && !entries.length && <Feedback kind="empty" title="Ready for a task."><p>What would you like to work on?</p></Feedback>}
    {turnPresentation(entries).map(node => {
      if (node.kind === 'process') return <TurnProcess key={node.key} id={node.key} open tools={node.process.tool_call_count} messages={node.process.message_count} outcome={node.process.outcome} running={node.process.outcome === 'running'} start={node.process.started_at ?? undefined} end={node.process.ended_at ?? undefined}/>;
      if (node.kind === 'tail') return <TurnTail key={node.key} text={node.text} response={node.response} latest={node.response === latestResponse?.completed_response} onHistorical={onHistorical} disabled={historicalDisabled} lineageSwitchSafe={lineageSwitchSafe}/>;
      const entry = node.entry;
      if (entry.item.type === 'attempt_terminal') return null;
      const statuses = statusesAt(placement, { messageId: entry.item.type === 'message' ? entry.item.message.id : undefined, cursor: entry.cursor });
      const key = process.membership.get(entry.cursor);
      const group = key ? process.groups.get(key) : undefined;
      const processOpen = !group || group.owner.outcome !== 'completed' || expanded.has(key!);
      const final = entry.item.type === 'message' && entry.item.message.id === entry.turn_process?.final_message_id;
      const statusOwner = (status: typeof statuses[number]) => process.attempts.get(JSON.stringify([snapshot.conversation_id, status.attempt_id]));
      const visibleStatus = statuses.some(status => { const owner = statusOwner(status); return !owner || process.groups.get(owner)?.owner.outcome !== 'completed' || expanded.has(owner); });
      const body = hasBody(entry);
      const displayed = final && entry.item.type === 'message' && entry.item.message.role === 'assistant'
        ? { ...entry.item.message, content: entry.item.message.content.filter(b => b.type !== 'reasoning') } : entry.item.type === 'message' ? entry.item.message : undefined;
      const entrySeat = group?.seat?.kind === 'entry' && group.seat.cursor === entry.cursor;
      const statusSeat = statuses.some(status => { const owner = statusOwner(status); const seat = owner ? process.groups.get(owner)?.seat : undefined; return seat?.kind === 'status' && seat.statusId === status.status_message_id; });
      if (!body && !statuses.length && !entrySeat) return null;
      return <Fragment key={entryIdentity(entry)}><div hidden={(!body || !!group && !processOpen && !final) && !entrySeat && !statusSeat && !visibleStatus} key={entryIdentity(entry)} data-chat-anchor-key={entryIdentity(entry)} data-turn-process-owner={key} data-response-reveal={entry === latestResponse || entry === latestUser ? 'always' : 'hover'}>
        {entrySeat && disclosure(key!)}
        {entry.completed_response && !process.groups.has(process.attempts.get(JSON.stringify([entry.completed_response.origin.conversation_id, entry.completed_response.origin.attempt_id])) ?? '') && <TurnProcess id={JSON.stringify([entry.completed_response.origin.conversation_id, entry.completed_response.origin.attempt_id])} open tools={entry.turn_process?.tool_call_count ?? 0} messages={entry.turn_process?.message_count ?? 0} durationMs={entry.completed_response.timing?.total_duration_ms ?? undefined}/>}
        {final && processOpen && entry.item.type === 'message' && entry.item.message.role === 'assistant' && <Content blocks={entry.item.message.content.filter(b => b.type === 'reasoning')} markdown/>}
        <div hidden={!final && !processOpen}>
        {body && (entry.item.type === 'message' ? <Message message={displayed!} actions={entry.item.type === 'message' && entry.item.message.role === 'user' && (!entry.item.message.kind || entry.item.message.kind === 'message') && <div className={tailCss.actions} aria-label="Message actions"><MessageTime time={entry.item.message.timestamp}/><CopyMessage text={entry.item.message.content.flatMap(block => block.type === 'text' ? [block.text] : []).join('')}/></div>} tools={(entry.tool_calls ?? []).map(tool => tool.state.type === 'settled' ? tool : snapshot.attempt?.foreground?.find(live => live.message_id === tool.message_id && live.block_index === tool.block_index && live.call_id === tool.call_id && live.tool_id === tool.tool_id) ?? tool)} /> : <details>
          <summary>{entry.item.type === 'publication_audit' ? 'Assistant recovery details' : 'Historical interaction details'}</summary>
          <pre>{json(entry.item)}</pre>
        </details>)}
        </div>
        {statuses.map(status => {
          // Keep the native anchor, while sharing its exact completed Attempt's
          // disclosure. Never borrow the adjacent row's ownership for a status.
          const owner = statusOwner(status);
          const seat = owner ? process.groups.get(owner)?.seat : undefined;
          return <div key={status.status_message_id}>{seat?.kind === 'status' && seat.statusId === status.status_message_id && disclosure(owner!)}<div hidden={!!owner && process.groups.get(owner)?.owner.outcome === 'completed' && !expanded.has(owner)}><AgentStatusAnnotation status={status}/></div></div>;
        })}
      </div>

      </Fragment>;
    })}
    {!!currentContext.length && <details><summary>Current context</summary>{currentContext.map(message => <Message key={message.id} message={message} />)}</details>}
    {snapshot.attempt && snapshot.attempt.phase.type !== 'settled' && !entries.some(entry => entry.turn_process?.conversation_id === snapshot.conversation_id && entry.turn_process.attempt_id === snapshot.attempt!.attempt_id) && !entries.some(entry => entry.completed_response?.origin.conversation_id === snapshot.conversation_id && entry.completed_response.origin.attempt_id === snapshot.attempt!.attempt_id) && <TurnProcess id={JSON.stringify([snapshot.conversation_id, snapshot.attempt.attempt_id])} open tools={snapshot.attempt.foreground?.length ?? 0} messages={0}
      running
      start={snapshot.transcript.statistics?.latest_turn?.attempt_id === snapshot.attempt.attempt_id ? snapshot.transcript.statistics.latest_turn.started_at : undefined}
      end={snapshot.transcript.statistics?.latest_turn?.attempt_id === snapshot.attempt.attempt_id ? snapshot.transcript.statistics.latest_turn.ended_at ?? undefined : undefined}/>}
    {streaming && !durableIds.has(streaming.message_id) && <div data-chat-anchor-key={`message:${streaming.message_id}`}><AssistantMessage label="Streaming response"><Content blocks={streaming.blocks ?? []} markdown streaming tools={snapshot.attempt?.foreground?.filter(tool => tool.message_id === streaming.message_id)}/></AssistantMessage></div>}
  </div>;
}
