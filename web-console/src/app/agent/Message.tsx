import { InboundMessage } from './InboundMessage';
import { useConversationPreferences } from '../conversation-preferences';
import { useTranslation } from '../../locale/react';
import type { CompactionMarker, MessageBlock, UserContentBlock, AssistantContentBlock, InFlightBlock } from '../../../../protocol/app-server/v43';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { Artifact } from '../components/Artifact';
import { UserMessage, AssistantMessage, CompactionMessage } from '../../presentation/agent/Message';
import { Reasoning } from '../../presentation/agent/Reasoning';
import { Tool } from './Tool';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v43';
import type { ReactNode } from 'react';

export function Content({ blocks, markdown = false, streaming = false, tools = [], reasoningHidden = false, include }: { reasoningHidden?: boolean; tools?: ForegroundToolExecution[]; markdown?: boolean; streaming?: boolean; blocks: (UserContentBlock | AssistantContentBlock | InFlightBlock)[]; include?: readonly number[] }) {
  const tx = useTranslation();
  const [preferences] = useConversationPreferences();
  return blocks.map((block, index) => {
    // A step piece renders listed blocks at their canonical indices, which Tool lookup keys on.
    if (include && !include.includes(index)) return null;
    if (block.type === 'text') return markdown ? <MarkdownText key={index} text={block.text} streaming={streaming} /> : <span key={index}>{block.text}</span>;
    if (block.type === 'uploaded_file') return <AttachmentCard key={`${block.batch_id}/${block.name}/${index}`} name={block.name} image={false} />;
    if (block.type === 'image' || block.type === 'file') return <Artifact key={block.artifact_id} id={block.artifact_id} image={block.type === 'image'} mimeType={block.type === 'file' ? block.mime_type ?? undefined : undefined} name={(block.type === 'image' ? block.alt : block.name) ?? undefined} />;
    if (block.type === 'reasoning') return <Reasoning key={index} text={block.text ?? ''} running={streaming} hidden={reasoningHidden} preview={streaming || preferences.transcriptMode !== 'compact'}/>;
    if (block.type === 'refusal') return <p key={index}>{block.text}</p>;
    if (block.type === 'tool_call') {
      const id = 'call_id' in block ? block.call_id : block.id;
      const tool = tools.find(tool => tool.block_index === ('block_index' in block ? block.block_index : index) && tool.call_id === id && tool.tool_id === block.tool_id);
      return tool ? <Tool key={id} tool={tool}/> : <small key={id}>{tx('agent:message.assembling')}{' '}{block.name}…</small>;
    }
    return null;
  });
}
export function Message({ message, compaction, tools = [], actions, streaming = false, blocks, reasoningHidden = false, include }: { compaction?: CompactionMarker | null; reasoningHidden?: boolean; message: MessageBlock; tools?: ForegroundToolExecution[]; actions?: ReactNode; streaming?: boolean; blocks?: InFlightBlock[]; include?: readonly number[] }) {
  const tx = useTranslation();
  if (message.role === 'tool') return null; // Results belong to the native call projection, never paired here.
  if (message.role === 'user' && typeof message.kind === 'object' && 'compaction_summary' in message.kind) {
    if (!compaction) return <Content blocks={message.content} markdown/>;
    const checkpoint = compaction;
    return <CompactionMessage title={checkpoint.manual === true ? tx('agent:context.command-title') : tx('agent:context.succeeded')}
      summary={tx('agent:context.completed', { items: checkpoint.retired_messages, tokens: checkpoint.retired_tokens })}>
      <Content blocks={message.content} markdown/>
    </CompactionMessage>;
  }
  if (message.role === 'user' && message.kind && message.kind !== 'message') return <details><summary>{tx('agent:message.context')}{' '}{Object.keys(message.kind)[0]}</summary><Content blocks={message.content} markdown/></details>;
  if (message.role === 'user' && message.source !== 'human') return <InboundMessage message={message}/>;
  return message.role === 'user' ? <UserMessage label={tx('agent:message.your-message')} actions={actions}><Content blocks={message.content}/></UserMessage>
    : <AssistantMessage label={tx(streaming ? 'agent:agent-transcript.streaming-response' : 'agent:message.assistant-response')}><Content blocks={blocks ?? message.content} markdown tools={tools} streaming={streaming} reasoningHidden={reasoningHidden} include={include}/></AssistantMessage>;
}
