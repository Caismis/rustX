import { fixtures } from '../../protocol/app-server/fixtures';
import type { WorkflowSource } from '../../protocol/app-server/v57';

/** One native example source exactly as the App Server fixture returns it. */
export function exampleSource(id: string): WorkflowSource {
  const message = fixtures.find(entry => 'result' in entry && entry.id === `workflow-source-${id}`);
  if (!message || !('result' in message) || message.result?.type !== 'workflow_source') throw new Error(`missing ${id}`);
  return structuredClone(message.result.source as unknown as WorkflowSource);
}

export const EXAMPLES = ['typed_agent', 'specialized_agent', 'parallel_checks', 'human_plan', 'parallel_review', 'implement_and_review'] as const;
