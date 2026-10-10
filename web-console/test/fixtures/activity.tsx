// Explicit native-state gates. This fixture is not part of the production bundle.
import { useSyncExternalStore } from 'react';
import { createRoot } from 'react-dom/client';
import { Tool } from '../../src/app/agent/Tool';
import { ArtifactContext } from '../../src/app/components/Artifact';
import { ArtifactResources } from '../../src/client/artifacts';
import { AgentCard } from '../../src/app/components/ActivityCards';
import { RuntimeFacts } from '../../src/app/agent/Activity';
import type { RuntimeClientAgent } from '../../../protocol/app-server/v43';
import { Server, snapshot, childConversation } from '../fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
const server = new Server();
const s = snapshot();
const agent: RuntimeClientAgent = { title: 'Worker',
  agent_id: 'agent-worker', parent_agent_id: 'parent-agent', activation_id: 'activation-a', current_activation: 'activation-a',
  child_conversation_id: 'conversation-worker', agent: 'Worker', state: 'active', activation_state: 'running',
  definition_digest: 'definition', profile_digest: 'profile', started_at: '2026-09-25T00:00:00Z',
  observation: { attempt_id: null, revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 0, model_retries: 0, tool_executions: 0 } },
  workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' }, resource_state: 'none' },
};
s.agents = [agent];
s.jobs = [{ job_id: 'job-build', tool_id: 'tool-bash', tool_name: 'Build', state: 'running', bash: { command: 'printf authoritative-command', description: 'Check the build <safely>' } }];
server.snapshots.set('A', s);
server.handlers.set('agent/conversation', () => childConversation( { entries: [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: 'report', content: [{ type: 'text', text: '## Final report\nCanonical child output.' }] } } }] }));
server.handlers.set('agent/sendMessage', request => {
  if (request.method !== 'agent/sendMessage') throw new Error('Wrong operation');
  const resumed = agent.state === 'inactive';
  if (resumed) { agent.state = 'active'; agent.activation_state = 'running'; agent.current_activation = 'activation-b'; agent.activation_id = 'activation-b'; }
  return { type: 'agent_message', agent_id: agent.agent_id, activation_id: agent.activation_id, resumed };
});
server.handlers.set('agent/interrupt', () => { agent.state = 'inactive'; agent.current_activation = null; agent.activation_state = 'cancelled'; return { type: 'agent_wait', agent_id: agent.agent_id, activation_id: agent.activation_id, outcome: 'cancelled', agent }; });
server.handlers.set('agent/wait', () => ({ type: 'agent_wait', agent_id: agent.agent_id, activation_id: agent.current_activation, outcome: agent.current_activation ? 'succeeded' : null, agent }));
server.handlers.set('job/status', () => ({ type: 'job', job: s.jobs![0] }));
server.handlers.set('job/wait', () => ({ type: 'job', job: s.jobs![0] }));
server.handlers.set('job/cancel', () => { s.jobs![0] = { ...s.jobs![0], state: 'cancelled', result: { status: { type: 'cancelled', reason: 'user_requested', phase: 'during_execution' }, duration_ms: 1, content: [{ type: 'text', text: 'Process settled' }] } }; return { type: 'job', job: s.jobs![0] }; });
server.handlers.set('artifact/read', () => ({ type: 'artifact_bytes', data: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aZ1sAAAAASUVORK5CYII=' }));
await server.attached('A');
const artifacts = new ArtifactResources(server.client, 'A');
function Fixture() {
  const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
  return <div style={{ maxWidth: 760, margin: '24px auto', padding: 16 }}><h1>Jobs and Agents</h1><div id="subagent-header-actions"/><ArtifactContext.Provider value={artifacts}><Tool tool={{ message_id: "image-message", block_index: 0, call_id: "read-image", tool_id: "tool-read-image", name: "read_image", state: { type: "settled", arguments: '{"path":"sample.png"}', result: { status: { type: "success" }, duration_ms: 1, content: [{ type: "image", artifact_id: "artifact_1" }] } } }}/></ArtifactContext.Provider><AgentCard agent={state.views.A.snapshot!.agents![0]} client={server.client} sessionId="A"/><RuntimeFacts snapshot={state.views.A.snapshot!} client={server.client} sessionId="A"/></div>;
}
createRoot(document.getElementById('root')!).render(<Fixture/>);
