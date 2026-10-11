import type { RuntimeClientAgent } from '../../protocol/app-server/v44';
export const incrementalAgent = (activation_id = 'activation-1'): RuntimeClientAgent => ({ title: 'worker',
  agent_id: 'durable-child', parent_agent_id: 'parent', activation_id,
  current_activation: activation_id, child_conversation_id: 'child-conversation',
  agent: 'worker', definition_digest: 'definition', profile_digest: 'profile',
  state: 'active', activation_state: 'running', started_at: '2026-09-28T00:00:00Z',
  observation: { attempt_id: null, revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 0, model_retries: 0, tool_executions: 0 } },
  workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' }, resource_state: 'none' },
});
