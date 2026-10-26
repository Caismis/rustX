import type { RuntimeClientAgent, RuntimeClientSnapshot } from '../../protocol/app-server/v59';
import { workflowRun } from './workflow-data';
import { Server, snapshot } from './fixture';

export function workflowCatalogSnapshot(): RuntimeClientSnapshot {
  const value = snapshot(), run = workflowRun();
  run.workflow_id = 'code_quality_gate'; run.state = { type: 'settled', outcome: 'completed' };
  run.instances = ['explore', 'review', 'test'].map((node, index) => ({ ...run.instances[1],
    node, kind: 'agent', child: `workflow-activation-${index}`, child_agent_id: `workflow-agent-${index}`, state: run.state }));
  run.agents_consumed = 3;
  value.agents = []; value.workflows = { revision: '1', runs: [run], omitted_runs: 0 };
  return value;
}

export function workflowChildHeader(index: number): RuntimeClientAgent {
  return { title: 'Native child title', agent_id: `workflow-agent-${index}`, agent: 'reviewer', parent_agent_id: 'root',
    child_conversation_id: `child-conversation-${index}`, activation_id: `workflow-activation-${index}`,
    state: 'inactive', activation_state: 'succeeded', definition_digest: 'definition', profile_digest: 'profile',
    started_at: '2026-10-21T00:00:00Z', workspace: '/workspace/A',
    observation: { attempt_id: null, revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 1, model_retries: 0, tool_executions: 0 } } };
}

export function installWorkflowChildStatus(server: Server) {
  server.handlers.set('agent/status', request => {
    if (request.method !== 'agent/status') throw new Error('Unexpected request');
    const index = Number(request.params.agent_id.split('-').at(-1)), header = workflowChildHeader(index);
    return { type: 'domain_page', page: { cursor: '1', read_cut: server.client.getSnapshot().views.A.checkpoint!.read_cut,
      domain: `workflow_children.${header.activation_id}`, total: '1',
      items: [{ offset: '0', identity: header.agent_id, value: header as never, content: [], deferred: false }], next_offset: null } };
  });
}
