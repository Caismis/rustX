import { childConversation, type Server } from '../fixture';
import type { RuntimeClientSnapshot } from '../../../protocol/app-server/v77';
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
  server.handlers.set('agent/turns', request => {
    if (request.method !== 'agent/turns') throw new Error('Unexpected request');
    const index = Number(request.params.agent_id.split('-').at(-1));
    const cut = { conversation_id: workflowChildHeader(index).child_conversation_id, journal: '0', transcript: '0', mutation_revision: '0',
      ledger_through: '0', surface_revision: '0', transcript_total: '0', pending_through: '0', pending_total: '0' };
    return { type: 'conversation_turns', page: { cut, offset: 0, total: '0', turns: [] } };
  });
  window.workflowCatalogFixture = { refresh: () => server.client.refresh('A'),
    statusReads: () => server.requests.filter(row => row.request.method === 'agent/status').length };
}

declare global { interface Window { workflowCatalogFixture: { refresh(): Promise<void>; statusReads(): number } } }
