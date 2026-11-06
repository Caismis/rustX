import { fixtures } from '../../protocol/app-server/fixtures';
import type { WorkflowSource } from '../../protocol/app-server/workflow-documents.ts';

/** One native example source exactly as the App Server fixture returns it. */
export function exampleSource(id: string): WorkflowSource {
  const message = fixtures.find(entry => 'result' in entry && entry.id === `workflow-source-${id}`);
  if (!message || !('result' in message) || message.result?.type !== 'workflow_source') throw new Error(`missing ${id}`);
  const content = fixtures.find(entry => 'result' in entry && entry.id === `workflow-document-${id}`);
  if (!content || !('result' in content) || content.result?.type !== 'workflow_document_chunk') throw new Error(`missing ${id} bytes`);
  const document = JSON.parse(new TextDecoder().decode(Uint8Array.from(atob(content.result.chunk.data), character => character.charCodeAt(0))));
  return structuredClone({ ...message.result.source, document } as WorkflowSource);
}

export const EXAMPLES = ['typed_agent', 'parallel_checks', 'human_plan', 'parallel_review', 'implement_and_review'] as const;

/** parallel_review whose two outcomes are Tools no catalog describes, each
 * with a literal `status` argument: plain values the inspector edits. */
export function statusSource(): WorkflowSource {
  const source = exampleSource('parallel_review'), nodes = source.document!.block.nodes;
  for (const [id, status] of [['ready', 'ready'], ['revise', 'needs_revision']]) {
    nodes[id] = { type: 'tool', selector: { origin: 'builtin', name: 'unlisted' }, result: { type: 'text' }, arguments: { type: 'object', fields: { status: { type: 'literal', value: status } } } };
  }
  return source;
}
