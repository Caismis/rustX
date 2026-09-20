import type { MessageBlock, UserContentBlock, AssistantContentBlock, InFlightBlock } from '../../../../protocol/app-server/v14';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { AttachmentCard } from '../../presentation/attachments/AttachmentCard';
import { Artifact } from '../components/Artifact';
import { UserMessage, AssistantMessage } from '../../presentation/agent/Message';
import { Reasoning } from '../../presentation/agent/Reasoning';
import { Tool } from './Tool';
import type { ForegroundToolExecution } from '../../../../protocol/app-server/v14';
import type { ReactNode } from 'react';

export function Content({ blocks, markdown = false, streaming = false, tools = [] }: { tools?: ForegroundToolExecution[]; markdown?: boolean; streaming?: boolean; blocks: (UserContentBlock | AssistantContentBlock | InFlightBlock)[] }) {
  return blocks.map((block, index) => {
    if (block.type === 'text') return markdown ? <MarkdownText key={index} text={block.text} streaming={streaming} /> : <span key={index}>{block.text}</span>;
    if (block.type === 'uploaded_file') return <AttachmentCard key={`${block.batch_id}/${block.name}/${index}`} name={block.name} image={false} />;
    if (block.type === 'image' || block.type === 'file') return <Artifact key={block.artifact_id} id={block.artifact_id} image={block.type === 'image'} mimeType={block.type === 'file' ? block.mime_type ?? undefined : undefined} name={(block.type === 'image' ? block.alt : block.name) ?? undefined} />;
    if (block.type === 'reasoning') return <Reasoning key={index} text={block.text ?? ''} running={streaming}/>;
    if (block.type === 'refusal') return <p key={index}>{block.text}</p>;
    if (block.type === 'tool_call') {
      const id = 'call_id' in block ? block.call_id : block.id;
      const tool = tools.find(tool => tool.block_index === ('block_index' in block ? block.block_index : index) && tool.call_id === id && tool.tool_id === block.tool_id);
      return tool ? <Tool key={id} tool={tool}/> : <small key={id}>Assembling {block.name}…</small>;
    }
    return null;
  });
}
export function Message({ message, tools = [], actions }: { message: MessageBlock; tools?: ForegroundToolExecution[]; actions?: ReactNode }) {
  if (message.role === 'tool') return null; // Results belong to the native call projection, never paired here.
  if (message.role === 'user' && message.kind && message.kind !== 'message') return <details><summary>Context · {Object.keys(message.kind)[0]}</summary><Content blocks={message.content} markdown/></details>;
  return message.role === 'user' ? <UserMessage label="Your message" actions={actions}><Content blocks={message.content}/></UserMessage>
    : <AssistantMessage label="Assistant response"><Content blocks={message.content} markdown tools={tools}/></AssistantMessage>;
}
