import type { MessageBlock, UserContentBlock, AssistantContentBlock, InFlightBlock } from '../../../../protocol/app-server/v1';
import { json } from '../../bindings/projection';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { Artifact, ToolArtifacts } from './Artifact';
import { MessageItem } from './MessageItem';
import { ToolRow } from './ToolRow';

export function Content({ blocks, markdown = false, streaming = false }: { markdown?: boolean; streaming?: boolean; blocks: (UserContentBlock | AssistantContentBlock | InFlightBlock)[] }) {
  return blocks.map((block, index) => {
    if (block.type === 'text') return markdown ? <MarkdownText key={index} text={block.text} streaming={streaming} /> : <span key={index}>{block.text}</span>;
    if (block.type === 'image' || block.type === 'file') return <Artifact key={block.artifact_id} id={block.artifact_id} image={block.type === 'image'} name={(block.type === 'image' ? block.alt : block.name) ?? undefined} />;
    if (block.type === 'reasoning') return <details key={index}><summary>Reasoning</summary>{block.text}</details>;
    if (block.type === 'refusal') return <p key={index}>{block.text}</p>;
    if (block.type === 'tool_call') return <ToolRow key={'call_id' in block ? block.call_id : block.id} title={block.name} summary={`Tool call · ${'call_id' in block ? block.call_id : block.id}`} input={typeof block.arguments === 'string' ? block.arguments : json(block.arguments)} />;
    return null;
  });
}
export function Message({ message }: { message: MessageBlock }) {
  if (message.role === 'user' && message.kind && message.kind !== 'message') return <details><summary>Context · {Object.keys(message.kind)[0]}</summary><Content blocks={message.content} markdown /></details>;
  return <MessageItem user={message.role === 'user'} label={`${message.role} · ${message.id}`}>
    {message.role === 'tool' ? <div data-tool-call-id={message.tool_call_id}><ToolRow title={message.tool_id} summary={`${message.result.status.type} · ${message.tool_call_id}`} output={json(message.result)} /><ToolArtifacts result={message.result} /></div> : <Content blocks={message.content} markdown />}
  </MessageItem>;
}
