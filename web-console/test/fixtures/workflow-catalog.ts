import { childConversation, type Server } from '../fixture';
import type { RuntimeClientSnapshot } from '../../../protocol/app-server/v62';
import { installWorkflowChildStatus, workflowCatalogSnapshot, workflowChildHeader } from '../workflow-catalog-data';

export function workflowCatalogFixture(server: Server, current: RuntimeClientSnapshot) {
  current.attempt = null; current.agents = []; current.workflows = workflowCatalogSnapshot().workflows;
  installWorkflowChildStatus(server);
  server.handlers.set('agent/observe', request => {
    if (request.method !== 'agent/observe') throw new Error('Unexpected request');
    const index = Number(request.params.agent_id.split('-').at(-1)), header = workflowChildHeader(index);
    return childConversation({ entries: [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: `result-${index}`,
      content: [{ type: 'text', text: `## ${current.workflows.runs[0].instances[index].node} report\n\nReview complete. All checks were recorded.` }] } } }] }, header.agent_id, header.activation_id, header.child_conversation_id);
  });
  window.workflowCatalogFixture = { refresh: () => server.client.refresh('A'),
    statusReads: () => server.requests.filter(row => row.request.method === 'agent/status').length };
}

declare global { interface Window { workflowCatalogFixture: { refresh(): Promise<void>; statusReads(): number } } }
