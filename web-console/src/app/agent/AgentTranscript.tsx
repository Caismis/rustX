import { WorkflowRunPanel, workflowKey } from './WorkflowRunPanel';
import { InteractionAudit } from './InteractionAudit';
import { TurnUsage } from './UsageStats';
import { attemptOwnerKey } from '../../../../protocol/app-server/attempt-identity';
import { sameToolOccurrence } from '../../../../protocol/app-server/projection';
import { statusIdentity } from '../../../../protocol/app-server/projection';
import { isUserBubble } from './InboundMessage';
import { AgentConversationContext } from './subagent-context';
import { toolCard } from '../../bindings/tools';
import { useConversationPreferences } from '../conversation-preferences';
import { useTranslation } from '../../locale/react';
import { useContext, useState, type ReactNode } from 'react';
import { turnPresentation } from '../../bindings/turn-presentation';
import { compactionPlacement } from '../../bindings/compaction-placement';
import { isFinalTurnMessage, turnProcesses } from '../../bindings/turn-process';
import { stepActivity, stepGroups, type StepPiece } from '../../bindings/step-groups';
import { StepGroup } from '../../presentation/agent/StepGroup';
import { TurnError } from '../../presentation/agent/TurnError';
import { RunningStatus } from '../../presentation/agent/RunningStatus';
import { TurnProcess } from '../../presentation/agent/TurnProcess';
import type { RuntimeClientSnapshot, RuntimeClientMessageDescriptor, CompletedResponseView, RuntimeClientTranscriptEntry, MessageBlock, InFlightBlock, ForegroundToolExecution } from '../../../../protocol/app-server/v57';
import { conversation } from '../../bindings/projection';
import { agentStatusPlacement, isRuntimeStatusContext, statusesAt } from '../../bindings/agent-status';
import { TranscriptContinuation } from './TranscriptContinuation';
import { Feedback } from '../components/ConversationFeedback';
import { AgentStatusAnnotation } from './AgentStatus';
import { Message, Content } from './Message';
import { messageContinuation, useMessageBlocks } from './message-blocks';
import { AssistantMessage, UserMessage } from '../../presentation/agent/Message';
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
import { useContentSource } from './ObservationSource';
import { SourceContent } from './SourceContent';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import type { PublicationHead } from '../../client/publication-heads';
import type { RuntimeClientProjectionHeadView } from '../../../../protocol/app-server/v57';
/** Standalone Tool-result bodies are suppressed: the native call projection
 * already renders their content beside the call. Suppressing a body does not
 * erase the entry's authoritative transcript position, so a `PostToolBatch`
 * annotation anchored there still renders in place — as an annotation-only slot. */
const hasInlineBody = (entry: RuntimeClientTranscriptEntry) => entry.item.type === 'publication_audit'
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
export function AgentTranscript({ snapshot, history, publicationHeads = [], retrying = false, loadEarlier, loadLater, onHistorical, historicalDisabled, lineageSwitchSafe = false, forkPoint, onOpenSource, sourceDisabled, requestFeedback, compactions = [], contextActivity, workflows }: { snapshot: Pick<RuntimeClientSnapshot, 'messages' | 'attempt' | 'transcript' | 'statuses' | 'conversation_id'> & Partial<Pick<RuntimeClientSnapshot,'message_descriptors'|'publication_settlements'|'pending_interactions'>>; history?: TranscriptCache; publicationHeads?: readonly PublicationHead[]; retrying?: boolean; loadEarlier?: () => void; loadLater?: () => void | Promise<void>; onHistorical?: (action: ResponseAction, response: CompletedResponseView) => void; historicalDisabled?: boolean; lineageSwitchSafe?: boolean; forkPoint?: ForkPointInfo; onOpenSource?: (origin: ForkOrigin) => void; sourceDisabled?: boolean; requestFeedback?: RequestFeedback; compactions?: readonly { head: RuntimeClientProjectionHeadView; content: ReactNode }[]; contextActivity?: ReactNode; workflows?: RuntimeClientSnapshot['workflows'] }) {
  const tx = useTranslation();
  const receiver = useContext(AgentConversationContext);
  const isUser = (message: MessageBlock) => isUserBubble(message, receiver?.parent_agent_id);
  const userActions = (message: MessageBlock) => isUser(message) && message.role === 'user' && <div className={tailCss.actions} aria-label={tx('agent:agent-transcript.message-actions')}><MessageTime time={message.timestamp}/><CopyMessage messageId={message.id} text={message.content.flatMap(block => block.type === 'text' ? [block.text] : []).join('')}/></div>;
  const [{ transcriptMode }] = useConversationPreferences();
  const verbose = transcriptMode === 'verbose';
  const groupedLive = transcriptMode === 'compact' || transcriptMode === 'standard';
  const { source } = useContentSource();
  const hasBody = (entry: RuntimeClientTranscriptEntry) => !(entry.item.type === 'message' && isRuntimeStatusContext(entry.item.message)) && hasInlineBody(entry) || entry.item.type === 'publication_audit'
    && (source?.content ?? []).some(binding => binding.content_ref.readable_end > 0
      && binding.content_ref.source.type === 'durable_transcript_audit' && entry.item.type === 'publication_audit'
      && binding.content_ref.source.stream_id === entry.item.audit.stream_id
      && binding.content_ref.representation === 'utf8_text'
      && (binding.deferred || entry.item.audit.content.some(block => block.block_index === binding.content_ref.source.block_index
        && block.kind !== 'proposed_tool_call' && new TextEncoder().encode(block.text).length !== binding.content_ref.readable_end)));
  const canonical = conversation(snapshot);
  const original = transcriptPresentation(history, snapshot.transcript);
  const page = history?.page ?? snapshot.transcript;
  // Canonical block counts also discover new live messages alongside a frozen window.
  const allDescriptors = deferredToolDescriptors(snapshot.message_descriptors ?? [], page, [], false);
  const candidates = new Map([...canonical.messages, ...original.flatMap(entry => entry.item.type === 'message' ? [entry.item.message] : [])].map(message => [message.id, message]));
  const requests = [...candidates.values()].flatMap(message => { const request = messageContinuation(message, allDescriptors, page); return request ? [request] : []; });
  const blockPages = useMessageBlocks(requests);
  const enrich = (message: MessageBlock): MessageBlock => {
    const request = messageContinuation(message, allDescriptors, page), suffix = request && blockPages.blocks(request);
    return suffix && message.role !== 'tool' ? { ...message, content: [...message.content, ...suffix] } as MessageBlock : message;
  };
  const messages = canonical.messages.map(enrich), streaming = canonical.streaming;
  const loaded = original.map(entry => {
    if (entry.item.type !== 'message') return entry;
    const message = enrich(entry.item.message);
    return message === entry.item.message ? entry : { ...entry, item: { ...entry.item, message } };
  });
  const heads: RuntimeClientTranscriptEntry[] = history?.window ? [] : publicationHeads.filter(head => head.read_cut.read.conversation_id === snapshot.conversation_id && !loaded.some(entry => entry.cursor === head.transcript_cursor))
    .map(head => ({ cursor: head.transcript_cursor, item: { type: 'publication_audit', audit: head.audit } }));
  const entries = heads.length ? [...loaded, ...heads].sort((a,b) => BigInt(a.cursor) < BigInt(b.cursor) ? -1 : 1) : loaded;
  const descriptors = deferredToolDescriptors(snapshot.message_descriptors ?? [], history?.page ?? snapshot.transcript, entries, !!history?.window).filter(descriptor => !isRuntimeStatusContext(descriptor));
  const descriptorProcessSeats = new Map<string,string>();
  const loadedProcessOwners = new Set(entries.flatMap(entry=>entry.turn_process?[attemptOwnerKey(entry.turn_process)]:[]));
  for(const descriptor of descriptors){
    const owner=descriptor.turn_process;if(!owner)continue;
    const key=attemptOwnerKey(owner);
    if(!loadedProcessOwners.has(key)&&!descriptorProcessSeats.has(key))descriptorProcessSeats.set(key,canonicalLocatorKey(descriptor.locator));
  }
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [stepsOpen, setStepsOpen] = useState<ReadonlySet<string>>(new Set());

  const latestResponse = entries.filter(entry => entry.completed_response).at(-1);
  const latestPartial = snapshot.attempt?.phase.type === 'settled' ? entries.filter(entry => entry.item.type === 'publication_audit' && entry.item.audit.attempt_id === snapshot.attempt?.attempt_id).at(-1) : undefined;
  const latestUser = entries.filter(entry => entry.item.type === 'message' && isUser(entry.item.message)).at(-1);
  const durableIds = new Set(entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message.id] : []));
  const settledPublications = new Set([...entries.flatMap(entry => entry.item.type === 'publication_audit' ? [entry.item.audit.message_id] : []),...(snapshot.publication_settlements ?? []).flatMap(record => record.settlement.message_id ? [record.settlement.message_id] : [])]);
  // Placement is a property of the authoritative status window and the runtime
  // facts each composition carries, never of what this page happens to hold: an
  // anchor outside the loaded transcript simply draws nothing until it is paged in.
  const placement = agentStatusPlacement(snapshot.statuses);
  const process = turnProcesses(entries, placement, snapshot.conversation_id);
  const isAskAudit = askUserAudits(entries);
  const settledRequest = (entry: RuntimeClientTranscriptEntry) => {
    const item = entry.item;
    return item.type === 'interaction_settled' && entries.some(request => request.item.type === 'interaction_requested' && request.item.attempt_id === item.attempt_id && request.item.interaction_id === item.interaction_id);
  };
  const toldByCall = (entry: RuntimeClientTranscriptEntry) => isAskAudit(entry) || settledRequest(entry);
  const runs = workflows?.runs.filter(run => run.id.conversation_id === snapshot.conversation_id) ?? [];
  const workflowSeats = new Map<string, typeof runs>();
  const unseated = runs.filter(run => {
    const attemptId = run.id.origin.type === 'tool' ? run.id.origin.attempt_id : undefined;
    const entry = attemptId == null ? undefined : entries.find(entry => entry.turn_process?.conversation_id === run.id.conversation_id && entry.turn_process.attempt_id === attemptId && entry.tool_calls?.some(tool => tool.call_id === run.tool_call_id));
    if (!entry) return true;
    const list = workflowSeats.get(entry.cursor) ?? []; list.push(run); workflowSeats.set(entry.cursor, list); return false;
  });
  const workflowPanels = (runs: typeof unseated) => runs.map(run => <WorkflowRunPanel key={workflowKey(run.id)} run={run} pending={snapshot.pending_interactions}/>);
  const compactionBoundaries = new Set(compactions.flatMap(({ head }) => {
    const next = entries.find(entry => BigInt(entry.cursor) > BigInt(head.transcript_anchor));
    return next ? [next.cursor] : [];
  }));
  const steps = stepGroups(entries, entry => hasBody(entry) && !toldByCall(entry),
    entry => compactionBoundaries.has(entry.cursor) || entry.item.type === 'message' && !!requestFeedback?.messages.has(entry.item.message.id));
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
      return messageSegments({ role: 'assistant', id: partial.message_id, content: [] },
        partial.content.flatMap(block => block.kind === 'proposed_tool_call' ? [] : [{ type: block.kind, block_index: block.block_index, text: block.text }]), [], { stopped: true, publicationStream: partial.stream_id,
          actions: partial.content.some(block => block.kind === 'text' || block.kind === 'refusal') && <div className={tailCss.actions}><CopyMessage text={text} publication={partial}/><MessageTime time={partial.settled_at}/></div> });
    }
    if (entry.item.type === 'interaction_requested' || entry.item.type === 'interaction_settled') {
      const item = entry.item;
      const settlement = entries.find(candidate => candidate.item.type === 'interaction_settled' && candidate.item.attempt_id === item.attempt_id && candidate.item.interaction_id === item.interaction_id)?.item;
      return <InteractionAudit item={item} settlement={settlement?.type === 'interaction_settled' ? settlement : undefined}/>;
    }
    return null;
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
  const messageSegments = (value: MessageBlock, blocks: InFlightBlock[] | Extract<MessageBlock, {role:'assistant'}>['content'], tools: ForegroundToolExecution[], { streaming = false, stopped = false, actions, publicationStream }: { streaming?: boolean; stopped?: boolean; actions?: ReactNode; publicationStream?: string } = {}) => {
    const segments: {process:boolean; indices:number[]}[]=[];
    blocks.forEach((block,index)=>{
      const process=block.type==='reasoning'||block.type==='tool_call', previous=segments.at(-1);
      const blockIndex = 'block_index' in block ? block.block_index : index;
      if(previous?.process===process)previous.indices.push(blockIndex);else segments.push({process,indices:[blockIndex]});
    });
    const responseTail = segments.reduce((last,segment,index) => segment.process ? last : index, -1);
    return segments.map((segment,index)=>{
      const key=`message:${value.id}:${segment.indices[0]}`;
      // Harness seats interruption chrome with the response portion; only a
      // reasoning-only partial keeps it inside its process portion.
      const last = index === (responseTail < 0 ? segments.length - 1 : responseTail);
      const message=<Message message={value} publicationStream={publicationStream} blocks={blocks} include={segment.indices} streaming={streaming} stopped={stopped && last} actions={last ? actions : undefined} tools={tools}/>;
      return <div className={css.flowItem} data-chat-group-part={segment.process ? undefined : 'response'} key={key}>
        {segment.process ? <StepGroup id={key} counts={[]} open={stepsOpen.has(key)} direct={!groupedLive}
          {...(streaming ? liveGroup(tools, blocks.filter((block,index)=>segment.indices.includes('block_index' in block ? block.block_index : index)&&block.type==='reasoning').map(block=>'text' in block ? block.text ?? '' : '').at(-1)) : {})}
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
  const currentContext = messages.filter(message => message.role === 'user' && message.kind && message.kind !== 'message' && !durableIds.has(message.id) && !isRuntimeStatusContext(message));
  const runningAttempt = !history?.window && !!snapshot.attempt && ['admitted','running'].includes(snapshot.attempt.phase.type) ? snapshot.attempt : undefined;
  const latestTurn = snapshot.transcript.statistics?.latest_turn;
  const startedAt = runningAttempt && (latestTurn?.attempt_id != null && latestTurn.attempt_id === runningAttempt.attempt_id ? latestTurn.started_at
    : entries.find(entry => entry.turn_process?.conversation_id === snapshot.conversation_id && attemptOwnerKey(entry.turn_process)===attemptOwnerKey({conversation_id:snapshot.conversation_id,...runningAttempt}))?.turn_process?.started_at);
  const startTime = startedAt ? Date.parse(startedAt) : undefined;
  const descriptorOwner = (descriptor:RuntimeClientMessageDescriptor) => (descriptor.turn_process && descriptorProcessSeats.get(attemptOwnerKey(descriptor.turn_process))===canonicalLocatorKey(descriptor.locator) && <>
        <TurnProcess id={attemptOwnerKey(descriptor.turn_process)} open tools={descriptor.turn_process.tool_call_count} messages={descriptor.turn_process.message_count}
          outcome={descriptor.turn_process.outcome} running={descriptor.turn_process.outcome==='running'} start={descriptor.turn_process.started_at??undefined} end={descriptor.turn_process.ended_at??undefined}/>
      </>);
  const descriptorResponse = (descriptor:RuntimeClientMessageDescriptor) => (descriptor.completed_response && <div className={tailCss.actions} aria-label={tx('agent:turn-tail.completed-turn')} data-turn-tail={attemptOwnerKey(descriptor.completed_response.origin)}>
        {descriptor.completed_response.usage && <TurnUsage usage={descriptor.completed_response.usage} models={descriptor.completed_response.models??[]}/>}
        <MessageTime time={descriptor.completed_response.completed_at}/>
      </div>);
  const flow = [...compactionPlacement(turnPresentation(entries, forkPoint ? { ...(history?.page ?? snapshot.transcript), inherited_through: forkPoint.through } : undefined, snapshot.conversation_id), compactions, entries, snapshot.transcript, history),
      ...(!history?.window ? messages.filter(message => !durableIds.has(message.id) && message.role !== 'tool'
        && !(message.role === 'user' && message.kind && message.kind !== 'message')).map(message => ({kind:'canonical' as const,message})) : []),
      ...(streaming && !durableIds.has(streaming.message_id) && !settledPublications.has(streaming.message_id) ? [{ kind: 'streaming' as const, streaming }] : [])];
  const nodes: (typeof flow[number] | { kind: 'continuation' })[] = [...flow];
  const continuation = history?.window?.newer_cursor;
  if (continuation != null && loadLater) {
    const at = flow.findIndex(node => node.kind === 'entry' && BigInt(node.entry.cursor) > BigInt(continuation)
      || node.kind === 'process' && BigInt(node.process.control_cursor) > BigInt(continuation)
      || node.kind === 'canonical' || node.kind === 'streaming');
    nodes.splice(at < 0 ? nodes.length : at, 0, { kind: 'continuation' });
  }
  return <div className={css.column} aria-label={tx('agent:agent-transcript.canonical-conversation')}>
    {!retrying && !history?.initialLoading && history?.page.next_cursor != null && <div className={css.older}><button type="button" disabled={history.loading || !loadEarlier} onClick={loadEarlier}>{history.loading ? tx('agent:agent-transcript.loading-earlier') : tx('agent:agent-transcript.load-earlier')}</button></div>}
    {history?.error && <p role="alert">{history.error}</p>}
    {!retrying && !messages.length && !entries.length && !runs.length && <Feedback kind="empty" title={tx('agent:agent-transcript.ready-for-a-task')}><p>{tx('agent:agent-transcript.what-would-you-like-to-work-on')}</p></Feedback>}
    {nodes.map(node => {
      if (node.kind === 'compaction') return <div className={css.flowItem} key={`compaction:${node.marker.head.activation_id}`} data-chat-anchor-key={`compaction:${node.marker.head.activation_id}`} data-compaction-anchor={node.marker.head.transcript_anchor}>{node.marker.content}</div>;
      if (node.kind === 'continuation') return <TranscriptContinuation key="history-continuation" cursor={continuation!} loading={!!history?.loading} load={loadLater!}/>;
      if (node.kind === 'fork-point') return <ForkPoint key="fork-point" point={forkPoint!} disabled={sourceDisabled ?? historicalDisabled} onOpen={onOpenSource}/>;
      if (node.kind === 'canonical') {
        // Current canonical heads can advance before a source-owned transcript
        // window refresh. Render that native body without inventing a T row,
        // process membership, completion, or historical cut.
        const descriptor = snapshot.message_descriptors?.find(item => item.identity === node.message.id);
        const tools = descriptor?.tool_calls ?? snapshot.attempt?.foreground?.filter(tool => tool.message_id === node.message.id) ?? [];
        const childUser = !!receiver && isUser(node.message);
        return <div className={css.flowItem} data-chat-flow-kind={childUser ? 'user' : undefined} key={`message:${node.message.id}`}><MessageSeat key="seat" id={`message:${node.message.id}`} reveal={childUser ? 'always' : undefined}
          prefix={<>{requestFeedback?.messages.get(node.message.id)}{descriptor && descriptorOwner(descriptor)}{descriptor && descriptorResponse(descriptor)}</>}
          message={node.message.role !== 'assistant' ? node.message : undefined} tools={tools} actions={childUser && userActions(node.message)}
          other={node.message.role === 'assistant' ? messageSegments(node.message,node.message.content,tools) : undefined}/></div>;
      }
      if (node.kind === 'streaming') {
        const tools = snapshot.attempt?.foreground?.filter(tool => tool.message_id === node.streaming.message_id) ?? [];
        return <div className={css.flowItem} key={`message:${node.streaming.message_id}`}><MessageSeat key="seat" id={`message:${node.streaming.message_id}`}
          turnOwner={snapshot.attempt ? turnAnchor({conversation_id:snapshot.conversation_id,attempt_id:snapshot.attempt.attempt_id,attempt_id_source:snapshot.attempt.attempt_id_source}) : undefined}
          prefix={requestFeedback?.messages.get(node.streaming.message_id)} other={messageSegments({role:'assistant',id:node.streaming.message_id,content:[]}, node.streaming.blocks ?? [], tools, { streaming: true })}/></div>;
      }
      if (node.kind === 'process' && node.process.outcome === 'running') return null;
      if (node.kind === 'tail') return <TurnTail key={node.key} text={node.text} response={node.response} latest={node.response === latestResponse?.completed_response} onHistorical={onHistorical} disabled={historicalDisabled} lineageSwitchSafe={lineageSwitchSafe}/>;
      if (node.kind === 'process') return <div className={css.flowItem} data-chat-flow-kind="turn-process" key={node.key} data-chat-turn-owner={turnAnchor(node.process)} data-chat-anchor-key={entries.some(entry => entry.cursor === node.process.control_cursor) ? turnAnchor(node.process) : undefined}><TurnProcess id={node.key} open tools={node.process.tool_call_count} messages={node.process.message_count} outcome={node.process.outcome} start={node.process.started_at ?? undefined} end={node.process.ended_at ?? undefined}/>{requestFeedback?.attempts.get(attemptOwnerKey(node.process))}</div>;
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
      const runPanels = workflowPanels(workflowSeats.get(entry.cursor) ?? []);
      const feedback = entry.item.type === 'message' ? requestFeedback?.messages.get(entry.item.message.id)
        : entry.item.type === 'publication_audit' ? requestFeedback?.messages.get(entry.item.audit.message_id) : undefined;
      // An entry whose rows all render in an earlier step group keeps no seat of its own.
      if ((!body || stepped?.length === 0) && !statuses.length && !entrySeat && !delivery && !feedback && !runPanels.length) return null;
      const seatHidden = (!body || stepped?.length === 0 || !!group && !processOpen && !final) && !entrySeat && !statusSeat && !visibleStatus;
      const response = entry.item.type === 'message' && entry.item.message.role === 'assistant' && (stepped ? stepped.some(piece => piece.kind === 'reply') : entry.item.message.content.some(block => block.type === 'text' || block.type === 'refusal'));
      return <div className={css.flowItem} hidden={seatHidden && !delivery && !feedback && !runPanels.length} data-chat-group-part={response ? 'response' : undefined} data-chat-flow-kind={entry.item.type === 'message' && isUser(entry.item.message) ? 'user' : undefined} key={entry.item.type === 'publication_audit' ? `message:${entry.item.audit.message_id}` : entryIdentity(entry)} data-chat-anchor-key={entry.turn_process?.outcome==='completed' && entry.turn_process.control_cursor===entry.cursor ? turnAnchor(entry.turn_process) : undefined}>{delivery && entry.item.type === 'message' && entry.item.message.role === 'tool' && <ToolDeliveries conversationId={history?.page.source_cut?.read.conversation_id??snapshot.conversation_id} locator={{type:'by_message_id',message_id:entry.item.message.id}} result={entry.item.message.result}/>}<MessageSeat key="seat" id={entryIdentity(entry)} turnOwner={entry.turn_process ? turnAnchor(entry.turn_process) : undefined} hidden={seatHidden && !feedback} owner={key} reveal={entry === latestResponse || entry === latestPartial || entry === latestUser ? 'always' : 'hover'}
        prefix={<>
        {entrySeat && disclosure(key!)}
        {entry.completed_response && !process.groups.has(process.attempts.get(attemptOwnerKey(entry.completed_response.origin)) ?? '') && <><TurnProcess id={attemptOwnerKey(entry.completed_response.origin)} open tools={entry.turn_process?.tool_call_count ?? 0} messages={entry.turn_process?.message_count ?? 0} durationMs={entry.completed_response.timing?.total_duration_ms ?? undefined}/>{requestFeedback?.attempts.get(attemptOwnerKey(entry.completed_response.origin))}</>}
        {feedback}
        </>} bodyHidden={!final && !processOpen}
        message={body && !stepped && entry.item.type === 'message' && entry.item.message.role !== 'assistant' ? entry.item.message : undefined} reasoningHidden={final && !processOpen}
        actions={entry.item.type === 'message' && userActions(entry.item.message)}
        tools={toolsOf(entry)}
        other={stepped ? pieces(entry, stepped, processOpen) : body && (entry.item.type === 'message' && entry.item.message.role === 'assistant' ? messageSegments(entry.item.message, entry.item.message.content, toolsOf(entry)) : audit(entry))}
        suffix={<>{statuses.map(status => {
          // Keep the native anchor, while sharing its exact completed Attempt's
          // disclosure. Never borrow the adjacent row's ownership for a status.
          const owner = statusOwner(status);
          const seat = owner ? process.groups.get(owner)?.seat : undefined;
          return <div className={css.flow} hidden={!verbose && !!owner && process.groups.get(owner)?.owner.outcome === 'completed' && !expanded.has(owner) && !(seat?.kind === 'status' && seat.statusId === statusIdentity(status))} key={statusIdentity(status)}>{seat?.kind === 'status' && seat.statusId === statusIdentity(status) && disclosure(owner!)}<div hidden={!verbose && !!owner && process.groups.get(owner)?.owner.outcome === 'completed' && !expanded.has(owner)}><AgentStatusAnnotation status={status}/></div></div>;
        })}</>}/>{runPanels}</div>;
    })}
    {blockPages.errors.map(error => <p role="alert" key={error}>{error} <button type="button" onClick={blockPages.retry}>{tx('agent:message.retry-content')}</button></p>)}
    {descriptors.filter(descriptor=>descriptor.deliveries && (descriptor.identity==null||!durableIds.has(descriptor.identity))).map(descriptor=><DeliveryHead key={`deliveries:${canonicalLocatorKey(descriptor.locator)}`} head={descriptor.deliveries!} messageIdentity={descriptor.identity}/>)}
    {descriptors.filter(descriptor => history?.window || descriptor.identity == null || !messages.some(message => message.id === descriptor.identity)).map(descriptor => <section key={canonicalLocatorKey(descriptor.locator)} data-canonical-locator={JSON.stringify(descriptor.locator)}>
      {descriptorOwner(descriptor)}
      <DescriptorContent descriptor={descriptor}/>
      {descriptorResponse(descriptor)}
      {descriptor.tool_calls.map(tool=><Tool key={`${canonicalLocatorKey(tool.occurrence.locator)}:${tool.block_index}`} tool={tool}/>)}
    </section>)}
    {!!currentContext.length && <details><summary>{tx('agent:agent-transcript.current-context')}</summary>{currentContext.map(message => <Message key={message.id} message={message} />)}</details>}
    {runningAttempt && requestFeedback?.attempts.get(attemptOwnerKey({conversation_id:snapshot.conversation_id,...runningAttempt}))}
    {retrying && <RunningStatus/>}
    {runningAttempt && <RunningStatus key={attemptOwnerKey({conversation_id:snapshot.conversation_id,...runningAttempt})} startTime={startTime !== undefined && Number.isFinite(startTime) ? startTime : undefined}/>}
    {workflowPanels(unseated)}
    {!!workflows?.omitted_runs && <p>{tx('common:workflow.omitted-runs', { count: workflows.omitted_runs })}</p>}
    {contextActivity}
  </div>;
}

function DescriptorContent({ descriptor }: { descriptor: RuntimeClientMessageDescriptor }) {
  const { source, read } = useContentSource(), tx = useTranslation();
  const hasBlocks = descriptor.collections.some(frontier => frontier.domain.endsWith('.blocks'));
  const request = { locator: descriptor.locator, offset: 0, total: descriptor.block_count };
  const native = useMessageBlocks(hasBlocks ? [request] : []), blocks = hasBlocks ? native.blocks(request) : undefined;
  const bindings = descriptor.content.filter(binding => binding.content_ref.representation === 'utf8_text' && binding.content_ref.source.component === 'payload');
  if (!source || !hasBlocks && !bindings.length) return null;
  if (native.errors.length) return <p role="alert">{native.errors.join("\n")} <button type="button" onClick={native.retry}>{tx('agent:message.retry-content')}</button></p>;
  if (blocks) {
    const attachments = blocks.flatMap((block,index) => ['uploaded_file','image','file'].includes(block.type) ? [index] : []);
    return descriptor.role === 'user' ? <UserMessage label={tx('agent:message.your-message')} attachments={<Content blocks={blocks} include={attachments} messageAttachments/>}><Content blocks={blocks} include={blocks.map((_,index)=>index).filter(index=>!attachments.includes(index))}/></UserMessage>
      : <AssistantMessage label={tx('agent:message.assistant-response')}><Content messageId={descriptor.identity ?? undefined} blocks={blocks} include={blocks.flatMap((block,index)=>block.type==='tool_call'?[]:[index])} markdown/></AssistantMessage>;
  }
  const content = <SourceContent explicit bindings={bindings} owned={source.owned} read={read}>{text => <MarkdownText text={text}/>}</SourceContent>;
  return descriptor.role === 'user' ? <UserMessage label={tx('agent:message.your-message')}>{content}</UserMessage>
    : <AssistantMessage label={tx('agent:message.assistant-response')}>{content}</AssistantMessage>;
}
