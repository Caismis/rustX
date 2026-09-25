// Explicit native-state gates. This fixture is not part of the production bundle.
import { useSyncExternalStore } from 'react';
import { createRoot } from 'react-dom/client';
import { RuntimeFacts } from '../../src/app/agent/Activity';
import type { RuntimeClientAgent } from '../../../protocol/app-server/v24';
import { Server, snapshot } from '../fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
const server = new Server();
const s = snapshot();
const agent: RuntimeClientAgent = {
  agent_id: 'agent-worker', parent_agent_id: 'parent-agent', activation_id: 'activation-a', current_activation: 'activation-a',
  child_conversation_id: 'conversation-worker', agent: 'Worker', state: 'active', activation_state: 'running',
  definition_digest: 'definition', profile_digest: 'profile', started_at: '2026-09-25T00:00:00Z',
  observation: { revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 0, model_retries: 0, tool_executions: 0 } },
  workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' }, resource_state: 'none' },
};
s.agents = [agent];
s.jobs = [{ job_id: 'job-build', tool_id: 'tool-bash', tool_name: 'Build', state: 'running' }];
server.snapshots.set('A', s);
server.handlers.set('agent/transcript', () => ({ type: 'transcript', page: { entries: [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: 'report', content: [{ type: 'text', text: '## Final report\nCanonical child output.' }] } } }] } }));
server.handlers.set('agent/sendMessage', request => {
  if (request.method !== 'agent/sendMessage') throw new Error('Wrong operation');
  const resumed = agent.state === 'inactive';
  if (resumed) { agent.state = 'active'; agent.activation_state = 'running'; agent.current_activation = 'activation-b'; agent.activation_id = 'activation-b'; }
  return { type: 'agent_message', agent_id: agent.agent_id, activation_id: agent.activation_id, resumed };
});
server.handlers.set('agent/interrupt', () => { agent.state = 'inactive'; agent.current_activation = null; agent.activation_state = 'cancelled'; return { type: 'agent', agent }; });
server.handlers.set('agent/wait', () => ({ type: 'agent_wait', agent_id: agent.agent_id, activation_id: agent.current_activation, agent }));
server.handlers.set('job/status', () => ({ type: 'job', job: s.jobs![0] }));
server.handlers.set('job/wait', () => ({ type: 'job', job: s.jobs![0] }));
server.handlers.set('job/cancel', () => { s.jobs![0] = { ...s.jobs![0], state: 'cancelled', result: { status: { type: 'cancelled', reason: 'user_requested', phase: 'during_execution' }, duration_ms: 1, content: [{ type: 'text', text: 'Process settled' }] } }; return { type: 'job', job: s.jobs![0] }; });
await server.attached('A');
function Fixture() {
  const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
  return <div style={{ maxWidth: 760, margin: '24px auto', padding: 16 }}><h1>Jobs and Agents</h1><RuntimeFacts snapshot={state.views.A.snapshot!} client={server.client} sessionId="A"/></div>;
}
createRoot(document.getElementById('root')!).render(<Fixture/>);
