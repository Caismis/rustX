import { isRuntimeStatusContext } from '../../bindings/agent-status';
import { InboundMessage, isUserBubble } from './InboundMessage';
import { AgentConversationContext } from './subagent-context';
import { useConversationPreferences } from '../conversation-preferences';
import { useTranslation } from '../../locale/react';
import type { MessageBlock, UserContentBlock, AssistantContentBlock, InFlightBlock } from '../../../../protocol/app-server/v57';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { Artifact } from '../components/Artifact';
import { UserMessage, AssistantMessage, MessageFile } from '../../presentation/agent/Message';
import { Reasoning } from '../../presentation/agent/Reasoning';
import { Tool } from './Tool';
import { CanonicalTool } from './CanonicalTool';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v57';
import { Fragment, useContext, type ReactNode } from 'react';
import { messageContentBinding, useContentSource } from './ObservationSource';
import { SourceContent } from './SourceContent';

export function Content({ blocks, messageId, publicationStream, markdown = false, streaming = false, tools = [], reasoningHidden = false, include, messageAttachments = false }: { messageId?: string; publicationStream?: string; reasoningHidden?: boolean; tools?: ForegroundToolExecution[]; markdown?: boolean; streaming?: boolean; blocks: (UserContentBlock | AssistantContentBlock | InFlightBlock)[]; include?: readonly number[]; messageAttachments?: boolean }) {
  const tx = useTranslation();
  const [preferences] = useConversationPreferences();
  return blocks.map((block, index) => {
    // A step piece renders listed blocks at their canonical indices, which Tool lookup keys on.
    const blockIndex = 'block_index' in block ? block.block_index : index;
    if (include && !include.includes(blockIndex)) return null;
    if (block.type === 'text') return <MessageText key={blockIndex} messageId={messageId} publicationStream={publicationStream} index={blockIndex} text={block.text} markdown={markdown} streaming={streaming}/>;
    if (block.type === 'uploaded_file') return <MessageFile key={`${block.batch_id}/${block.name}/${index}`} name={block.name}/>;
    if (block.type === 'image' || block.type === 'file') return <Artifact key={block.artifact_id} id={block.artifact_id} image={block.type === 'image'} mimeType={block.type === 'file' ? block.mime_type ?? undefined : undefined} name={(block.type === 'image' ? block.alt : block.name) ?? undefined} messageAttachment={messageAttachments}/>;
    if (block.type === 'reasoning') return <MessageText key={blockIndex} messageId={messageId} publicationStream={publicationStream} index={blockIndex} text={block.text ?? ''} markdown={markdown} streaming={streaming} renderText={text => <Reasoning text={text} running={streaming} hidden={reasoningHidden} preview={streaming || preferences.transcriptMode !== 'compact'}/>}/>;
    if (block.type === 'refusal') return <MessageText key={blockIndex} messageId={messageId} publicationStream={publicationStream} index={blockIndex} text={block.text} markdown={markdown} streaming={streaming} renderText={text => <p>{text}</p>}/>;
    if (block.type === 'tool_call') {
      const indexInMessage='block_index' in block?block.block_index:index;
      const id = 'block_index' in block ? block.call_id : block.id;
      const tool = tools.find(tool => tool.block_index === indexInMessage && (id==null||tool.call_id==null||tool.call_id===id) && (block.tool_id==null||tool.tool_id==null||tool.tool_id===block.tool_id));
      return tool ? <Tool key={indexInMessage} tool={tool}/> : messageId && !streaming ? <CanonicalTool key={indexInMessage} messageId={messageId} blockIndex={indexInMessage} name={block.name}/> : <small key={indexInMessage}>{tx('agent:message.assembling')}{' '}{block.name}…</small>;
    }
    return null;
  });
}

/** Separate attachment and text seats without changing canonical block indices. */
function UserMessageContent({ message, label, actions }: { message: Extract<MessageBlock, { role: 'user' }>; label: string; actions?: ReactNode }) {
  const { source } = useContentSource();
  const attachments: number[] = [], text: number[] = [];
  message.content.forEach((block, index) => {
    if (block.type !== 'text') attachments.push(index);
    else if (block.text !== '' || (messageContentBinding(source, message.id, index)?.content_ref.readable_end ?? 0) > 0) text.push(index);
  });
  return <UserMessage label={label} actions={actions} attachments={attachments.length > 0 ? <Content messageId={message.id} blocks={message.content} include={attachments} messageAttachments/> : undefined}>
    {text.length > 0 ? text.map((index, position) => <Fragment key={index}>{position > 0 && '\n\n'}<Content messageId={message.id} blocks={message.content} include={[index]}/></Fragment>) : undefined}
  </UserMessage>;
}
function MessageText({ messageId, publicationStream, index, text, markdown, streaming, renderText }: { messageId?: string; publicationStream?: string; index: number; text: string; markdown: boolean; streaming: boolean; renderText?: (text: string) => ReactNode }) {
  const { source, read } = useContentSource(), binding = messageId ? messageContentBinding(source, messageId, index, publicationStream) : undefined;
  const render = renderText ?? ((value: string) => markdown ? <MarkdownText text={value} streaming={streaming}/> : <span>{value}</span>);
  // Native metadata describes its own header. The browser's finite live-byte
  // cache can omit an otherwise inline native field, so compare actual bytes.
  const deferred = binding && (binding.deferred || new TextEncoder().encode(text).length !== binding.content_ref.readable_end);
  return binding && deferred && source ? <SourceContent bindings={[{...binding,deferred:true}]} owned={source.owned} read={read} fallback={render(text)}>{render}</SourceContent> : render(text);
}
export function Message({ message, publicationStream, tools = [], actions, streaming = false, stopped = false, blocks, reasoningHidden = false, include }: { reasoningHidden?: boolean; message: MessageBlock; publicationStream?: string; tools?: ForegroundToolExecution[]; actions?: ReactNode; streaming?: boolean; stopped?: boolean; blocks?: (AssistantContentBlock | InFlightBlock)[]; include?: readonly number[] }) {
  const tx = useTranslation(), receiver = useContext(AgentConversationContext);
  if (message.role === 'tool' || isRuntimeStatusContext(message)) return null; // Results belong to the native call projection, never paired here.
  if (message.role === 'user' && message.kind && message.kind !== 'message') return <details><summary>{tx('agent:message.context')}{' '}{Object.keys(message.kind)[0]}</summary><Content messageId={message.id} blocks={message.content} markdown/></details>;
  if (message.role === 'user' && !isUserBubble(message, receiver?.parent_agent_id)) return <InboundMessage message={message}/>;
  return message.role === 'user' ? <UserMessageContent message={message} label={tx(message.source === 'human' ? 'agent:message.your-message' : 'common:subagents.parent-message')} actions={actions}/>
    : <AssistantMessage label={tx(stopped ? 'agent:message.incomplete-response' : streaming ? 'agent:agent-transcript.streaming-response' : 'agent:message.assistant-response')} stopped={stopped ? tx('agent:turn-process.stopped') : undefined} actions={actions}><Content messageId={message.id} publicationStream={publicationStream} blocks={blocks ?? message.content} markdown tools={tools} streaming={streaming} reasoningHidden={reasoningHidden} include={include}/></AssistantMessage>;
}
