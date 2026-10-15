import { TurnUsage } from './UsageStats';
import { attemptOwnerKey } from '../../../../protocol/app-server/attempt-identity';
import { sameToolOccurrence } from '../../../../protocol/app-server/projection';
import { statusIdentity } from '../../../../protocol/app-server/projection';
import { isHumanMessage } from './InboundMessage';
import { AttemptIdentityValue, AttemptJournalValues, TurnFinalMessageValue } from './JournalValue';
import { toolCard } from '../../bindings/tools';
import { useConversationPreferences } from '../conversation-preferences';
import { useTranslation } from '../../locale/react';
import { useState, type ReactNode } from 'react';
import { turnPresentation } from '../../bindings/turn-presentation';
import { isFinalTurnDescriptor, isFinalTurnMessage, turnProcesses } from '../../bindings/turn-process';
import { stepActivity, stepGroups, type StepPiece } from '../../bindings/step-groups';
import { StepGroup } from '../../presentation/agent/StepGroup';
import { TurnError } from '../../presentation/agent/TurnError';
import { RunningStatus } from '../../presentation/agent/RunningStatus';
import { TurnProcess } from '../../presentation/agent/TurnProcess';
import type { RuntimeClientSnapshot, RuntimeClientMessageDescriptor, CompletedResponseView, RuntimeClientTranscriptEntry, MessageBlock, InFlightBlock, ForegroundToolExecution } from '../../../../protocol/app-server/v50';
import { conversation, json } from '../../bindings/projection';
import { agentStatusPlacement, isAgentStatusContext, statusesAt } from '../../bindings/agent-status';
import { Button } from '../../presentation/primitives/Button';
import { Feedback } from '../components/ConversationFeedback';
import { AgentStatusAnnotation } from './AgentStatus';
import { Content, Message } from './Message';
import { AssistantMessage } from '../../presentation/agent/Message';
import { ForkPoint, type ForkPointInfo, type ForkOrigin } from './ForkPoint';
import { ToolDeliveries } from '../components/Artifact';
import { DeliveryHead } from './DeliveryHead';
import { Tool } from './Tool';
import { deferredToolDescriptors } from '../../bindings/deferred-tools';
import { canonicalLocatorKey } from '../../../../protocol/app-server/content-identity';
import { transcriptPresentation, entryIdentity, turnAnchor, type TranscriptCache } from '../../client/transcript';
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
  return <div className={css.flow} hidden={hidden} data-chat-anchor-key={id} data-chat-turn-owner={turnOwner} data-turn-process-owner={owner} data-response-reveal={reveal}>
    {prefix}<div className={css.flow} hidden={bodyHidden}>{message ? <Message message={message} tools={tools} actions={actions} blocks={blocks} streaming={streaming} reasoningHidden={reasoningHidden}/> : other}</div>{suffix}
  </div>;
}
export interface RequestFeedback {
  messages: ReadonlyMap<string, ReactNode>;
  attempts: ReadonlyMap<string, ReactNode>;
}
export function AgentTranscript({ snapshot, history, loadEarlier, loadLater, onHistorical, historicalDisabled, lineageSwitchSafe = false, forkPoint, onOpenSource, sourceDisabled, requestFeedback }: { snapshot: Pick<RuntimeClientSnapshot, 'messages' | 'attempt' | 'transcript' | 'statuses' | 'conversation_id'> & Partial<Pick<RuntimeClientSnapshot,'message_descriptors'|'publication_settlements'>>; history?: TranscriptCache; loadEarlier?: () => void; loadLater?: () => void; onHistorical?: (action: ResponseAction, response: CompletedResponseView) => void; historicalDisabled?: boolean; lineageSwitchSafe?: boolean; forkPoint?: ForkPointInfo; onOpenSource?: (origin: ForkOrigin) => void; sourceDisabled?: boolean; requestFeedback?: RequestFeedback }) {
  const tx = useTranslation();
  const [{ transcriptMode }] = useConversationPreferences();
  const verbose = transcriptMode === 'verbose';
  const groupedLive = transcriptMode === 'compact' || transcriptMode === 'standard';
  const { messages, streaming } = conversation(snapshot);
  const entries = transcriptPresentation(history, snapshot.transcript);
  const descriptors = deferredToolDescriptors(snapshot.message_descriptors ?? [], history?.page ?? snapshot.transcript, entries, !!history?.window);
  const descriptorProcessSeats = new Map<string,string>();
  const processIdentitySeats = new Map<string,string>();
  const completedOwners = new Set(entries.flatMap(entry=>entry.completed_response?[attemptOwnerKey(entry.completed_response.origin)]:[]));
  for(const entry of entries)if(entry.turn_process?.attempt_id_source){
    const owner=attemptOwnerKey(entry.turn_process);
    if(!completedOwners.has(owner)&&!processIdentitySeats.has(owner))processIdentitySeats.set(owner,entry.cursor);
  }
  const loadedProcessOwners = new Set(entries.flatMap(entry=>entry.turn_process?[attemptOwnerKey(entry.turn_process)]:[]));
  for(const descriptor of descriptors){
    const owner=descriptor.turn_process;if(!owner)continue;
    const key=attemptOwnerKey(owner);
    if(!loadedProcessOwners.has(key)&&!descriptorProcessSeats.has(key))descriptorProcessSeats.set(key,canonicalLocatorKey(descriptor.locator));
  }
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [stepsOpen, setStepsOpen] = useState<ReadonlySet<string>>(new Set());

  const latestResponse = entries.filter(entry => entry.completed_response).at(-1);
  const latestUser = entries.filter(entry => entry.item.type === 'message' && isHumanMessage(entry.item.message)).at(-1);
  const durableIds = new Set(entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []));
  const settledPublications = new Set([...entries.flatMap(entry => entry.item.type === 'publication_audit' ? [entry.item.audit.message_id] : []),...(snapshot.publication_settlements ?? []).flatMap(record => record.settlement.message_id ? [record.settlement.message_id] : [])]);
  // Placement is a property of the authoritative status window and the runtime
  // facts each composition carries, never of what this page happens to hold: an
  // anchor outside the loaded transcript simply draws nothing until it is paged in.
  const placement = agentStatusPlacement(snapshot.statuses);
  const process = turnProcesses(entries, placement, snapshot.conversation_id);
  const toldByCall = askUserAudits(entries);
  const steps = stepGroups(entries, entry => hasBody(entry) && !toldByCall(entry),
    entry => entry.item.type === 'message' && !!requestFeedback?.messages.has(entry.item.message.id));
  const toolsOf = (entry: RuntimeClientTranscriptEntry) => (entry.tool_calls ?? []).map(tool => history?.window || tool.state.type === 'settled' ? tool : snapshot.attempt?.foreground?.find(live => sameToolOccurrence(live, tool) && (tool.metadata_ref == null || live.metadata_ref == null || live.metadata_ref.conversation_id === tool.metadata_ref.conversation_id)) ?? tool);
  const liveGroup = (tools: readonly ForegroundToolExecution[], reasoning?: string) => {
    const active = tools.filter(tool => tool.state.type !== 'settled').at(-1);
    const kind = active ? stepActivity(active.name) : 'thinking';
    return { activityTitle: tx(`agent:step-process.live.${kind}`), detail: transcriptMode === 'standard' ? (active ? toolCard(active).summary : reasoning?.split('\n').find(line => line.trim())) : undefined };
  };
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
      direct={verbose || !groupedLive && entry.turn_process?.outcome === 'running'}
      {...(entry.turn_process?.outcome === 'running' ? liveGroup(piece.group.members.flatMap(member => toolsOf(member.entry)), piece.group.members.flatMap(member => member.entry.item.type === 'message' && member.entry.item.message.role === 'assistant' ? member.entry.item.message.content.filter(block => block.type === 'reasoning').map(block => block.text ?? '') : []).at(-1)) : {})}
      onToggle={() => setStepsOpen(previous => { const next = new Set(previous); if (next.has(piece.group.key)) next.delete(piece.group.key); else next.add(piece.group.key); return next; })}>
      {piece.group.members.map(member => <div key={entryIdentity(member.entry)}>{member.blocks && member.entry.item.type === 'message'
        ? <Message message={member.entry.item.message} include={member.blocks} tools={toolsOf(member.entry)}/> : audit(member.entry)}</div>)}
    </StepGroup>);
  // A canonical correction of a message without a published process keeps
  // the same seats, block keys and disclosure state as its streaming form.
  const messageSegments = (value: MessageBlock, blocks: InFlightBlock[] | Extract<MessageBlock, {role:'assistant'}>['content'], tools: ForegroundToolExecution[], streaming = false) => {
    const segments: {process:boolean; indices:number[]}[]=[];
    blocks.forEach((block,index)=>{
      const process=block.type==='reasoning'||block.type==='tool_call', previous=segments.at(-1);
      if(previous?.process===process)previous.indices.push(index);else segments.push({process,indices:[index]});
    });
    return segments.map(segment=>{
      const key=`message:${value.id}:${segment.indices[0]}`;
      const message=<Message message={value} blocks={streaming ? blocks as InFlightBlock[] : undefined} include={segment.indices} streaming={streaming} tools={tools}/>;
      return <div className={css.flowItem} data-chat-group-part={segment.process ? undefined : 'response'} key={key}>
        {segment.process ? <StepGroup id={key} counts={[]} open={stepsOpen.has(key)} direct={!groupedLive}
          {...(streaming ? liveGroup(tools, blocks.filter((block,index)=>segment.indices.includes(index)&&block.type==='reasoning').map(block=>'text' in block ? block.text ?? '' : '').at(-1)) : {})}
          onToggle={()=>setStepsOpen(previous=>{const next=new Set(previous);if(next.has(key))next.delete(key);else next.add(key);return next;})}>{message}</StepGroup> : message}
      </div>;
    });
  };
  const disclosure = (key: string) => {
    const group = process.groups.get(key)!;
    const response = entries.find(entry => entry.turn_process && attemptOwnerKey(entry.turn_process) === key && entry.completed_response)?.completed_response;
    return <><TurnProcess durationMs={response?.timing?.total_duration_ms ?? undefined} id={key} open={verbose || expanded.has(key)} tools={group.tools} messages={group.messages} toggle={verbose ? undefined : () => setExpanded(previous => {
      const next = new Set(previous); if (next.has(key)) next.delete(key); else next.add(key); return next;
    })}/>{requestFeedback?.attempts.get(attemptOwnerKey(group.owner))}</>;
  };
  // Agent Status has its own anchored annotation, so its canonical Context message
  // must not reappear here as ordinary chat or as purported current context.
  const currentContext = messages.filter(message => message.role === 'user' && message.kind && message.kind !== 'message' && !durableIds.has(message.id) && !isAgentStatusContext(message));
  const runningAttempt = !history?.window && !!snapshot.attempt && ['admitted','running'].includes(snapshot.attempt.phase.type) ? snapshot.attempt : undefined;
  const latestTurn = snapshot.transcript.statistics?.latest_turn;
  const startedAt = runningAttempt && (latestTurn?.attempt_id != null && latestTurn.attempt_id === runningAttempt.attempt_id ? latestTurn.started_at
    : entries.find(entry => entry.turn_process?.conversation_id === snapshot.conversation_id && attemptOwnerKey(entry.turn_process)===attemptOwnerKey({conversation_id:snapshot.conversation_id,...runningAttempt}))?.turn_process?.started_at);
  const startTime = startedAt ? Date.parse(startedAt) : undefined;
  const descriptorOwner = (descriptor:RuntimeClientMessageDescriptor) => (descriptor.turn_process && descriptorProcessSeats.get(attemptOwnerKey(descriptor.turn_process))===canonicalLocatorKey(descriptor.locator) && <>
        <TurnProcess id={attemptOwnerKey(descriptor.turn_process)} open tools={descriptor.turn_process.tool_call_count} messages={descriptor.turn_process.message_count}
          outcome={descriptor.turn_process.outcome} running={descriptor.turn_process.outcome==='running'} start={descriptor.turn_process.started_at??undefined} end={descriptor.turn_process.ended_at??undefined}/>
        <AttemptIdentityValue field={descriptor.turn_process.attempt_id_source}/>
        {descriptor.turn_process.outcome==='completed' && descriptor.turn_process.final_message_id_deferred && <TurnFinalMessageValue reference={descriptor.turn_process.final_message_id_source}/>}
      </>);
  const descriptorResponse = (descriptor:RuntimeClientMessageDescriptor) => (descriptor.completed_response && <div className={tailCss.actions} aria-label={tx('agent:turn-tail.completed-turn')} data-turn-tail={attemptOwnerKey(descriptor.completed_response.origin)}>
        {!descriptor.turn_process && <AttemptIdentityValue field={descriptor.completed_response.origin.attempt_id_source}/>}
        {descriptor.completed_response.usage && <TurnUsage usage={descriptor.completed_response.usage} models={descriptor.completed_response.models??[]}/>}
        <MessageTime time={descriptor.completed_response.completed_at}/>
      </div>);
  return <div className={css.column} aria-label={tx('agent:agent-transcript.canonical-conversation')}>
    {!history?.window && <AttemptJournalValues attempt={snapshot.attempt}/>}
    {history?.page.next_cursor != null && <Button disabled={history.loading} onClick={loadEarlier}>{history.loading ? tx('agent:agent-transcript.loading-earlier') : tx('agent:agent-transcript.load-earlier')}</Button>}
    {history?.window && <p role="status">{tx('agent:agent-transcript.history-window')}</p>}
    {history?.window?.newer_cursor != null && <Button disabled={history.loading} onClick={loadLater}>{tx('agent:agent-transcript.load-later')}</Button>}
    {history?.error && <p role="alert">{history.error}</p>}
    {!messages.length && !entries.length && <Feedback kind="empty" title={tx('agent:agent-transcript.ready-for-a-task')}><p>{tx('agent:agent-transcript.what-would-you-like-to-work-on')}</p></Feedback>}
    {[...turnPresentation(entries, forkPoint ? { ...(history?.page ?? snapshot.transcript), inherited_through: forkPoint.through } : undefined),
      ...(!history?.window ? messages.filter(message => !durableIds.has(message.id) && message.role !== 'tool'
        && !(message.role === 'user' && message.kind && message.kind !== 'message')).map(message => ({kind:'canonical' as const,message})) : []),
      ...(streaming && !durableIds.has(streaming.message_id) && !settledPublications.has(streaming.message_id) ? [{ kind: 'streaming' as const, streaming }] : [])].map(node => {
      if (node.kind === 'fork-point') return <ForkPoint key="fork-point" point={forkPoint!} disabled={sourceDisabled ?? historicalDisabled} onOpen={onOpenSource}/>;
      if (node.kind === 'canonical') {
        // Current canonical heads can advance before a source-owned transcript
        // window refresh. Render that native body without inventing a T row,
        // process membership, completion, or historical cut.
        const descriptor = snapshot.message_descriptors?.find(item => item.identity === node.message.id);
        const tools = descriptor?.tool_calls ?? snapshot.attempt?.foreground?.filter(tool => tool.message_id === node.message.id) ?? [];
        return <div className={css.flowItem} key={`message:${node.message.id}`}><MessageSeat key="seat" id={`message:${node.message.id}`}
          prefix={<>{requestFeedback?.messages.get(node.message.id)}{descriptor && descriptorOwner(descriptor)}{descriptor && descriptorResponse(descriptor)}</>}
          message={node.message.role !== 'assistant' ? node.message : undefined} tools={tools}
          other={node.message.role === 'assistant' ? messageSegments(node.message,node.message.content,tools) : undefined}/></div>;
      }
      if (node.kind === 'streaming') {
        const tools = snapshot.attempt?.foreground?.filter(tool => tool.message_id === node.streaming.message_id) ?? [];
        return <div className={css.flowItem} key={`message:${node.streaming.message_id}`}><MessageSeat key="seat" id={`message:${node.streaming.message_id}`}
          turnOwner={snapshot.attempt ? turnAnchor({conversation_id:snapshot.conversation_id,attempt_id:snapshot.attempt.attempt_id,attempt_id_source:snapshot.attempt.attempt_id_source}) : undefined}
          prefix={requestFeedback?.messages.get(node.streaming.message_id)} other={messageSegments({role:'assistant',id:node.streaming.message_id,content:[]}, node.streaming.blocks ?? [], tools, true)}/></div>;
      }
      if (node.kind === 'process' && node.process.outcome === 'running') return null;
      if (node.kind === 'process') return <div className={css.flowItem} data-chat-flow-kind="turn-process" key={node.key} data-chat-turn-owner={turnAnchor(node.process)} data-chat-anchor-key={entries.some(entry => entry.cursor === node.process.control_cursor) ? turnAnchor(node.process) : undefined}><TurnProcess id={node.key} open tools={node.process.tool_call_count} messages={node.process.message_count} outcome={node.process.outcome} start={node.process.started_at ?? undefined} end={node.process.ended_at ?? undefined}/>{requestFeedback?.attempts.get(attemptOwnerKey(node.process))}{!processIdentitySeats.has(node.key) && <AttemptIdentityValue field={node.process.attempt_id_source}/>}</div>;
      if (node.kind === 'tail') return <TurnTail key={node.key} text={node.text} response={node.response} latest={node.response === latestResponse?.completed_response} onHistorical={onHistorical} disabled={historicalDisabled} lineageSwitchSafe={lineageSwitchSafe}/>;
      const entry = node.entry;
      if (entry.item.type === 'attempt_terminal') {
        const turn = entry.item.turn;
        if (turn.outcome !== 'failed') return null;
        const failure = turn.failure;
        const error = failure?.type === 'runtime' ? failure.error : undefined;
        const code = failure?.type === 'model' ? failure.kind : error?.type;
        const message = failure?.type === 'model'
          ? failure.kind === 'authentication' ? tx('agent:failure.auth') : failure.message
          : error ? 'message' in error ? error.message : 'reason' in error ? error.reason : tx('agent:failure.unknown-tool', { name: error.name })
            : tx('agent:failure.unavailable');
        return <div key={entryIdentity(entry)} data-chat-anchor-key={entryIdentity(entry)} data-chat-turn-owner={turnAnchor(turn)}>
          <TurnError title={tx('agent:failure.turn-error')} message={message} code={code}/>
        </div>;
      }
      const statuses = statusesAt(placement, { messageId: entry.item.type === 'message' ? entry.item.message.id : undefined, cursor: entry.cursor });
      const key = process.membership.get(entry.cursor);
      const group = key ? process.groups.get(key) : undefined;
      const processOpen = verbose || !group || group.owner.outcome !== 'completed' || expanded.has(key!);
      const final = isFinalTurnMessage(entry);
      const statusOwner = (status: typeof statuses[number]) => process.attempts.get(JSON.stringify([snapshot.conversation_id, status.attempt_id]));
      const visibleStatus = statuses.some(status => { const owner = statusOwner(status); return verbose || !owner || process.groups.get(owner)?.owner.outcome !== 'completed' || expanded.has(owner); });
      const body = hasBody(entry) && !toldByCall(entry);
      const stepped = steps.get(entry.cursor);
      const entrySeat = group?.seat?.kind === 'entry' && group.seat.cursor === entry.cursor;
      const statusSeat = statuses.some(status => { const owner = statusOwner(status); const seat = owner ? process.groups.get(owner)?.seat : undefined; return seat?.kind === 'status' && seat.statusId === statusIdentity(status); });
      const delivery = entry.item.type === 'message' && entry.item.message.role === 'tool' && entry.item.message.result.status.type === 'success' && !!entry.item.message.result.deliveries?.length;
      const feedback = entry.item.type === 'message' ? requestFeedback?.messages.get(entry.item.message.id)
        : entry.item.type === 'publication_audit' ? requestFeedback?.messages.get(entry.item.audit.message_id) : undefined;
      // An entry whose rows all render in an earlier step group keeps no seat of its own.
      if ((!body || stepped?.length === 0) && !statuses.length && !entrySeat && !delivery && !feedback) return null;
      const seatHidden = (!body || stepped?.length === 0 || !!group && !processOpen && !final) && !entrySeat && !statusSeat && !visibleStatus;
      const response = entry.item.type === 'message' && entry.item.message.role === 'assistant' && (stepped ? stepped.some(piece => piece.kind === 'reply') : entry.item.message.content.some(block => block.type === 'text' || block.type === 'refusal'));
      return <div className={css.flowItem} hidden={seatHidden && !delivery && !feedback} data-chat-group-part={response ? 'response' : undefined} data-chat-flow-kind={entry.item.type === 'message' && isHumanMessage(entry.item.message) ? 'user' : undefined} key={entryIdentity(entry)} data-chat-anchor-key={entry.turn_process?.outcome==='completed' && entry.turn_process.control_cursor===entry.cursor ? turnAnchor(entry.turn_process) : undefined}>{delivery && entry.item.type === 'message' && entry.item.message.role === 'tool' && <ToolDeliveries conversationId={history?.page.source_cut?.read.conversation_id??snapshot.conversation_id} locator={{type:'by_message_id',message_id:entry.item.message.id}} result={entry.item.message.result}/>}<MessageSeat key="seat" id={entryIdentity(entry)} turnOwner={entry.turn_process ? turnAnchor(entry.turn_process) : undefined} hidden={seatHidden && !feedback} owner={key} reveal={entry === latestResponse || entry === latestUser ? 'always' : 'hover'}
        prefix={<>
        {entrySeat && disclosure(key!)}
        {entry.completed_response && !process.groups.has(process.attempts.get(attemptOwnerKey(entry.completed_response.origin)) ?? '') && <><TurnProcess id={attemptOwnerKey(entry.completed_response.origin)} open tools={entry.turn_process?.tool_call_count ?? 0} messages={entry.turn_process?.message_count ?? 0} durationMs={entry.completed_response.timing?.total_duration_ms ?? undefined}/>{requestFeedback?.attempts.get(attemptOwnerKey(entry.completed_response.origin))}</>}
        {feedback}
        {entry.turn_process && processIdentitySeats.get(attemptOwnerKey(entry.turn_process))===entry.cursor && <AttemptIdentityValue field={entry.turn_process.attempt_id_source}/>}
        {entry.turn_process && process.finalMetadataSeats.get(attemptOwnerKey(entry.turn_process))===entry.cursor && <TurnFinalMessageValue reference={entry.turn_process.final_message_id_source}/>}
        </>} bodyHidden={!final && !processOpen}
        message={body && !stepped && entry.item.type === 'message' && entry.item.message.role !== 'assistant' ? entry.item.message : undefined} reasoningHidden={final && !processOpen}
        actions={entry.item.type === 'message' && isHumanMessage(entry.item.message) && (!entry.item.message.kind || entry.item.message.kind === 'message') && <div className={tailCss.actions} aria-label={tx('agent:agent-transcript.message-actions')}><MessageTime time={entry.item.message.timestamp}/><CopyMessage text={entry.item.message.content.flatMap(block => block.type === 'text' ? [block.text] : []).join('')}/></div>}
        tools={toolsOf(entry)}
        other={stepped ? pieces(entry, stepped, processOpen) : body && (entry.item.type === 'message' && entry.item.message.role === 'assistant' ? messageSegments(entry.item.message, entry.item.message.content, toolsOf(entry)) : audit(entry))}
        suffix={<>{statuses.map(status => {
          // Keep the native anchor, while sharing its exact completed Attempt's
          // disclosure. Never borrow the adjacent row's ownership for a status.
          const owner = statusOwner(status);
          const seat = owner ? process.groups.get(owner)?.seat : undefined;
          return <div className={css.flow} hidden={!verbose && !!owner && process.groups.get(owner)?.owner.outcome === 'completed' && !expanded.has(owner) && !(seat?.kind === 'status' && seat.statusId === statusIdentity(status))} key={statusIdentity(status)}>{seat?.kind === 'status' && seat.statusId === statusIdentity(status) && disclosure(owner!)}<div hidden={!verbose && !!owner && process.groups.get(owner)?.owner.outcome === 'completed' && !expanded.has(owner)}><AgentStatusAnnotation status={status}/></div></div>;
        })}</>}/></div>;
    })}
    {(snapshot.publication_settlements ?? []).filter(record => !entries.some(entry => entry.cursor === record.transcript_cursor)).map(record => <p role="status" key={`publication:${record.transcript_cursor}`} data-publication-settlement={record.settlement.kind}>{tx('agent:message.incomplete-response')} · {tx('common:activity.content-deferred')} ({record.settlement.block_count}) <MessageTime time={record.settlement.settled_at}/></p>)}
    {descriptors.filter(descriptor=>descriptor.deliveries && (descriptor.identity==null||!durableIds.has(descriptor.identity))).map(descriptor=><DeliveryHead key={`deliveries:${canonicalLocatorKey(descriptor.locator)}`} head={descriptor.deliveries!}/>)}
    {descriptors.filter(descriptor => history?.window || descriptor.identity == null || !messages.some(message => message.id === descriptor.identity)).map(descriptor => <section key={canonicalLocatorKey(descriptor.locator)} data-canonical-locator={JSON.stringify(descriptor.locator)}>
      {descriptorOwner(descriptor)}
      <p role="status" data-native-final-message={isFinalTurnDescriptor(descriptor)||undefined}>{tx('common:activity.content-deferred')} ({descriptor.role}, {descriptor.block_count})</p>
      {descriptorResponse(descriptor)}
      {descriptor.tool_calls.map(tool=><Tool key={`${canonicalLocatorKey(tool.occurrence.locator)}:${tool.block_index}`} tool={tool}/>)}
    </section>)}
    {!!currentContext.length && <details><summary>{tx('agent:agent-transcript.current-context')}</summary>{currentContext.map(message => <Message key={message.id} message={message} />)}</details>}
    {runningAttempt && requestFeedback?.attempts.get(attemptOwnerKey({conversation_id:snapshot.conversation_id,...runningAttempt}))}
    {snapshot.attempt?.attempt_id_source && <AttemptIdentityValue field={snapshot.attempt.attempt_id_source}/>}
    {runningAttempt && <RunningStatus key={attemptOwnerKey({conversation_id:snapshot.conversation_id,...runningAttempt})} startTime={startTime !== undefined && Number.isFinite(startTime) ? startTime : undefined}/>}
  </div>;
}
