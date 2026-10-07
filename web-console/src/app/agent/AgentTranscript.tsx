import { useTranslation } from '../../locale/react';
import { useState, type ReactNode } from 'react';
import { turnPresentation } from '../../bindings/turn-presentation';
import { turnProcesses } from '../../bindings/turn-process';
import { stepGroups, type StepPiece } from '../../bindings/step-groups';
import { StepGroup } from '../../presentation/agent/StepGroup';
import { TurnProcess } from '../../presentation/agent/TurnProcess';
import type { RuntimeClientSnapshot, CompletedResponseView, RuntimeClientTranscriptEntry, MessageBlock, InFlightBlock, ForegroundToolExecution } from '../../../../protocol/app-server/v36';
import { conversation, json } from '../../bindings/projection';
import { agentStatusPlacement, isAgentStatusContext, statusesAt } from '../../bindings/agent-status';
import { Button } from '../../presentation/primitives/Button';
import { Feedback } from '../components/ConversationFeedback';
import { AgentStatusAnnotation } from './AgentStatus';
import { Content, Message } from './Message';
import { AssistantMessage } from '../../presentation/agent/Message';
import { ToolDeliveries } from '../components/Artifact';
import { entryIdentity, turnAnchor, type TranscriptCache } from '../../client/transcript';
import type { ResponseAction } from '../commands/native';
import { CopyMessage, MessageTime, TurnTail } from './TurnTail';
import tailCss from './TurnTail.module.css';
import css from '../../presentation/agent/Chat.module.css';
/** Standalone Tool-result bodies are suppressed: the native call projection
 * already renders their content beside the call. Suppressing a body does not
 * erase the entry's authoritative transcript position, so a `PostToolBatch`
 * annotation anchored there still renders in place — as an annotation-only slot. */
const hasBody = (entry: RuntimeClientTranscriptEntry) => entry.item.type === 'publication_audit'
  ? entry.item.audit.content.some(block => block.kind !== 'proposed_tool_call' && block.text.trim() !== '')
  : entry.item.type !== 'message' || entry.item.message.role !== 'tool';
/** A native `ask_user` questionnaire is already told by its call's question row
 * (Harness shows no separate interaction record), so its request and settlement
 * audits keep their transcript positions without a body of their own. Other
 * questionnaires (MCP elicitation) keep their audits: their call rows are generic. */
function askUserAudits(entries: readonly RuntimeClientTranscriptEntry[]) {
  const asked = new Set(entries.flatMap(entry => entry.item.type === 'interaction_requested' && entry.item.subject.type === 'questionnaire' && entry.item.subject.requester.tool_id === 'tool-ask-user' ? [entry.item.interaction_id] : []));
  return (entry: RuntimeClientTranscriptEntry) => (entry.item.type === 'interaction_requested' || entry.item.type === 'interaction_settled') && asked.has(entry.item.interaction_id);
}
function MessageSeat({ id, hidden, owner, turnOwner, reveal, prefix, suffix, bodyHidden, message, tools, actions, blocks, streaming, reasoningHidden, other }: {
  id: string; hidden?: boolean; owner?: string; turnOwner?: string; reveal?: string; prefix?: ReactNode; suffix?: ReactNode; bodyHidden?: boolean;
  message?: MessageBlock; tools?: ForegroundToolExecution[]; actions?: ReactNode; blocks?: InFlightBlock[]; streaming?: boolean; reasoningHidden?: boolean; other?: ReactNode;
}) {
  return <div hidden={hidden} data-chat-anchor-key={id} data-chat-turn-owner={turnOwner} data-turn-process-owner={owner} data-response-reveal={reveal}>
    {prefix}<div hidden={bodyHidden}>{message ? <Message message={message} tools={tools} actions={actions} blocks={blocks} streaming={streaming} reasoningHidden={reasoningHidden}/> : other}</div>{suffix}
  </div>;
}
export function AgentTranscript({ snapshot, history, loadEarlier, onHistorical, historicalDisabled, lineageSwitchSafe = false }: { snapshot: Pick<RuntimeClientSnapshot, 'messages' | 'attempt' | 'transcript' | 'statuses' | 'conversation_id'>; history?: TranscriptCache; loadEarlier?: () => void; onHistorical?: (action: ResponseAction, response: CompletedResponseView) => void; historicalDisabled?: boolean; lineageSwitchSafe?: boolean }) {
  const tx = useTranslation();
  const { messages, streaming } = conversation(snapshot);
  const entries = history?.page.entries ?? snapshot.transcript.entries ?? [];
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [stepsOpen, setStepsOpen] = useState<ReadonlySet<string>>(new Set());

  const latestResponse = entries.filter(entry => entry.completed_response).at(-1);
  const latestUser = entries.filter(entry => entry.item.type === 'message' && entry.item.message.role === 'user').at(-1);
  const durableIds = new Set(entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []));
  const settledPublications = new Set(entries.flatMap(entry => entry.item.type === 'publication_audit' ? [entry.item.audit.message_id] : []));
  // Placement is a property of the authoritative status window and the runtime
  // facts each composition carries, never of what this page happens to hold: an
  // anchor outside the loaded transcript simply draws nothing until it is paged in.
  const placement = agentStatusPlacement(snapshot.statuses);
  const process = turnProcesses(entries, placement, snapshot.conversation_id);
  const toldByCall = askUserAudits(entries);
  const steps = stepGroups(entries, entry => hasBody(entry) && !toldByCall(entry));
  const toolsOf = (entry: RuntimeClientTranscriptEntry) => (entry.tool_calls ?? []).map(tool => tool.state.type === 'settled' ? tool : snapshot.attempt?.foreground?.find(live => live.message_id === tool.message_id && live.block_index === tool.block_index && live.call_id === tool.call_id && live.tool_id === tool.tool_id) ?? tool);
  // Publication audits are frozen released output, not diagnostic cards. Empty
  // failed requests (including overflow before compaction) have no Chat body.
  // Proposed calls are not executions and must never acquire Tool UI or actions.
  const audit = (entry: RuntimeClientTranscriptEntry) => {
    if (entry.item.type === 'message') return null;
    if (entry.item.type === 'publication_audit') {
      const partial = entry.item.audit;
      const text = partial.content.flatMap(block => block.kind === 'text' || block.kind === 'refusal' ? [block.text] : []).join('');
      return <AssistantMessage label={tx('agent:message.incomplete-response')}>
        <Content blocks={partial.content.flatMap(block => block.kind === 'proposed_tool_call' ? [] : [{ type: block.kind, block_index: block.block_index, text: block.text }])} markdown/>
        {text.trim() && <div className={tailCss.actions}><CopyMessage text={text}/><MessageTime time={partial.settled_at}/></div>}
      </AssistantMessage>;
    }
    return <details><summary>{tx('agent:agent-transcript.historical-interaction-details')}</summary><pre>{json(entry.item)}</pre></details>;
  };
  // A settled Attempt's steps render in place: replies stand alone, and each
  // process group renders whole in the entry where it starts.
  const pieces = (entry: RuntimeClientTranscriptEntry, list: readonly StepPiece[], processOpen: boolean) => list.map(piece => piece.kind === 'reply'
    ? entry.item.type === 'message' && <Message key={`reply:${piece.blocks[0]}`} message={entry.item.message} include={piece.blocks} tools={toolsOf(entry)}/>
    : <StepGroup key={piece.group.key} id={piece.group.key} counts={piece.group.counts} hidden={!processOpen} open={stepsOpen.has(piece.group.key)}
      onToggle={() => setStepsOpen(previous => { const next = new Set(previous); if (next.has(piece.group.key)) next.delete(piece.group.key); else next.add(piece.group.key); return next; })}>
      {piece.group.members.map(member => <div key={entryIdentity(member.entry)}>{member.blocks && member.entry.item.type === 'message'
        ? <Message message={member.entry.item.message} include={member.blocks} tools={toolsOf(member.entry)}/> : audit(member.entry)}</div>)}
    </StepGroup>);
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
  const liveProcess = snapshot.attempt && snapshot.attempt.phase.type !== 'settled' && !entries.some(entry => entry.turn_process?.conversation_id === snapshot.conversation_id && entry.turn_process.attempt_id === snapshot.attempt!.attempt_id) && !entries.some(entry => entry.completed_response?.origin.conversation_id === snapshot.conversation_id && entry.completed_response.origin.attempt_id === snapshot.attempt!.attempt_id) && <TurnProcess id={JSON.stringify([snapshot.conversation_id, snapshot.attempt.attempt_id])} open tools={snapshot.attempt.foreground?.length ?? 0} messages={0}
      running
      start={snapshot.transcript.statistics?.latest_turn?.attempt_id === snapshot.attempt.attempt_id ? snapshot.transcript.statistics.latest_turn.started_at : undefined}
      end={snapshot.transcript.statistics?.latest_turn?.attempt_id === snapshot.attempt.attempt_id ? snapshot.transcript.statistics.latest_turn.ended_at ?? undefined : undefined}/>;
  return <div className={css.column} aria-label={tx('agent:agent-transcript.canonical-conversation')}>
    {history?.page.next_cursor != null && <Button disabled={history.loading} onClick={loadEarlier}>{history.loading ? tx('agent:agent-transcript.loading-earlier') : tx('agent:agent-transcript.load-earlier')}</Button>}
    {history?.error && <p role="alert">{history.error}</p>}
    {!messages.length && !entries.length && <Feedback kind="empty" title={tx('agent:agent-transcript.ready-for-a-task')}><p>{tx('agent:agent-transcript.what-would-you-like-to-work-on')}</p></Feedback>}
    {[...turnPresentation(entries), ...(liveProcess ? [{ kind: 'live-process' as const }] : []), ...(streaming && !durableIds.has(streaming.message_id) && !settledPublications.has(streaming.message_id) ? [{ kind: 'streaming' as const, streaming }] : [])].map(node => {
      if (node.kind === 'live-process') return <div key="live-process" data-chat-turn-owner={turnAnchor({conversation_id:snapshot.conversation_id,attempt_id:snapshot.attempt!.attempt_id})}>{liveProcess}</div>;
      if (node.kind === 'streaming') return <div key={`message:${node.streaming.message_id}`}><MessageSeat key="seat" id={`message:${node.streaming.message_id}`} turnOwner={snapshot.attempt ? turnAnchor({conversation_id:snapshot.conversation_id,attempt_id:snapshot.attempt.attempt_id}) : undefined}
        message={{ role: 'assistant', id: node.streaming.message_id, content: [] }} blocks={node.streaming.blocks ?? []} streaming
        tools={snapshot.attempt?.foreground?.filter(tool => tool.message_id === node.streaming.message_id)}/></div>;
      if (node.kind === 'process') return <div key={node.key} data-chat-turn-owner={turnAnchor(node.process)} data-chat-anchor-key={entries.some(entry => entry.cursor === node.process.control_cursor) ? turnAnchor(node.process) : undefined}><TurnProcess id={node.key} open tools={node.process.tool_call_count} messages={node.process.message_count} outcome={node.process.outcome} running={node.process.outcome === 'running'} start={node.process.started_at ?? undefined} end={node.process.ended_at ?? undefined}/></div>;
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
      const body = hasBody(entry) && !toldByCall(entry);
      const stepped = steps.get(entry.cursor);
      const entrySeat = group?.seat?.kind === 'entry' && group.seat.cursor === entry.cursor;
      const statusSeat = statuses.some(status => { const owner = statusOwner(status); const seat = owner ? process.groups.get(owner)?.seat : undefined; return seat?.kind === 'status' && seat.statusId === status.status_message_id; });
      const delivery = entry.item.type === 'message' && entry.item.message.role === 'tool' && entry.item.message.result.status.type === 'success' && !!entry.item.message.result.deliveries?.length;
      // An entry whose rows all render in an earlier step group keeps no seat of its own.
      if ((!body || stepped?.length === 0) && !statuses.length && !entrySeat && !delivery) return null;
      return <div key={entryIdentity(entry)} data-chat-anchor-key={entry.turn_process?.outcome==='completed' && entry.turn_process.control_cursor===entry.cursor ? turnAnchor(entry.turn_process) : undefined}>{delivery && entry.item.type === 'message' && entry.item.message.role === 'tool' && <ToolDeliveries messageId={entry.item.message.id} result={entry.item.message.result}/>}<MessageSeat key="seat" id={entryIdentity(entry)} turnOwner={entry.turn_process ? turnAnchor(entry.turn_process) : undefined} hidden={(!body || stepped?.length === 0 || !!group && !processOpen && !final) && !entrySeat && !statusSeat && !visibleStatus} owner={key} reveal={entry === latestResponse || entry === latestUser ? 'always' : 'hover'}
        prefix={<>
        {entrySeat && disclosure(key!)}
        {entry.completed_response && !process.groups.has(process.attempts.get(JSON.stringify([entry.completed_response.origin.conversation_id, entry.completed_response.origin.attempt_id])) ?? '') && <TurnProcess id={JSON.stringify([entry.completed_response.origin.conversation_id, entry.completed_response.origin.attempt_id])} open tools={entry.turn_process?.tool_call_count ?? 0} messages={entry.turn_process?.message_count ?? 0} durationMs={entry.completed_response.timing?.total_duration_ms ?? undefined}/>}
        </>} bodyHidden={!final && !processOpen}
        message={body && !stepped && entry.item.type === 'message' ? entry.item.message : undefined} reasoningHidden={final && !processOpen}
        actions={entry.item.type === 'message' && entry.item.message.role === 'user' && (!entry.item.message.kind || entry.item.message.kind === 'message') && <div className={tailCss.actions} aria-label={tx('agent:agent-transcript.message-actions')}><MessageTime time={entry.item.message.timestamp}/><CopyMessage text={entry.item.message.content.flatMap(block => block.type === 'text' ? [block.text] : []).join('')}/></div>}
        tools={toolsOf(entry)}
        other={stepped ? pieces(entry, stepped, processOpen) : body && audit(entry)}
        suffix={<>{statuses.map(status => {
          // Keep the native anchor, while sharing its exact completed Attempt's
          // disclosure. Never borrow the adjacent row's ownership for a status.
          const owner = statusOwner(status);
          const seat = owner ? process.groups.get(owner)?.seat : undefined;
          return <div key={status.status_message_id}>{seat?.kind === 'status' && seat.statusId === status.status_message_id && disclosure(owner!)}<div hidden={!!owner && process.groups.get(owner)?.owner.outcome === 'completed' && !expanded.has(owner)}><AgentStatusAnnotation status={status}/></div></div>;
        })}</>}/></div>;
    })}
    {!!currentContext.length && <details><summary>{tx('agent:agent-transcript.current-context')}</summary>{currentContext.map(message => <Message key={message.id} message={message} />)}</details>}


  </div>;
}
