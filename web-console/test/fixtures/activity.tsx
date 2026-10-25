import { traceRecord } from '../trace-fixture';
import { domainReceipt, jobReceipt } from '../fixture';
// Explicit native-state gates. This fixture is not part of the production bundle.
import { useSyncExternalStore } from 'react';
import { createRoot } from 'react-dom/client';
import { Tool } from '../../src/app/agent/Tool';
import { ArtifactContext } from '../../src/app/components/Artifact';
import { ArtifactResources } from '../../src/client/artifacts';
import { AgentCard } from '../../src/app/components/ActivityCards';
import { RuntimeFacts } from '../runtime-facts';
import { BackgroundJobs } from '../../src/app/agent/BackgroundJobs';
import { InboundMessage } from '../../src/app/agent/InboundMessage';
import type { RuntimeClientAgent, RuntimeClientJob } from '../../../protocol/app-server/v57';
import { Server, snapshot, childConversation } from '../fixture';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/gradient-shadow-text.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
document.body.toggleAttribute('data-ds-dark-theme', new URLSearchParams(location.search).get('theme') === 'dark');
const server = new Server();
const s = snapshot();
const agent: RuntimeClientAgent = { title: 'Worker',
  agent_id: 'agent-worker', parent_agent_id: 'parent-agent', activation_id: 'activation-a', current_activation: 'activation-a',
  child_conversation_id: 'conversation-worker', agent: 'Worker', state: 'active', activation_state: 'running',
  definition_digest: 'definition', profile_digest: 'profile', started_at: '2026-09-25T00:00:00Z',
  observation: { attempt_id: null, revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 0, model_retries: 0, tool_executions: 0 } },
  workspace: '/workspace',
};
s.agents = [agent];
s.jobs = [{ job_id: 'job-build', tool_id: 'tool-bash', tool_name: 'Build', state: 'running', bash: { command: 'printf authoritative-command', description: 'Check the build <safely>' } }];
s.trace.records = [traceRecord(90, { kind: 'background', native_id: 'job-build', timing: { started_at: new Date(Date.now() - 27000).toISOString() } })];
server.snapshots.set('A', s);
server.handlers.set('agent/observe', () => childConversation( { entries: [{ cursor: '1', item: { type: 'message', message: { role: 'assistant', id: 'report', content: [{ type: 'text', text: '## Final report\nCanonical child output.' }] } } }] }, agent.agent_id, agent.activation_id, agent.child_conversation_id));
server.handlers.set('agent/sendMessage', request => {
  if (request.method !== 'agent/sendMessage') throw new Error('Wrong operation');
  const resumed = agent.state === 'inactive';
  if (resumed) { agent.state = 'active'; agent.activation_state = 'running'; agent.current_activation = 'activation-b'; agent.activation_id = 'activation-b'; void server.update('A', structuredClone(s)); }
  return { type: 'agent_message', agent_id: agent.agent_id, activation_id: agent.activation_id, resumed };
});
server.handlers.set('agent/interrupt', () => {
  const activation_id = agent.activation_id;
  // The fixture's native settlement publishes separately from its intent ACK.
  agent.state = 'inactive'; agent.current_activation = null; agent.activation_state = 'cancelled';
  void server.update('A', structuredClone(s));
  return { type: 'agent_interruption_accepted', agent_id: agent.agent_id, activation_id };
});
server.handlers.set('agent/wait', () => ({ type: 'agent_wait', agent_id: agent.agent_id, activation_id: agent.current_activation, outcome: agent.current_activation ? 'succeeded' : null }));
server.handlers.set('job/status', () => (domainReceipt('jobs.'+(s.jobs![0]).job_id,[s.jobs![0]])));
server.handlers.set('job/wait', () => (jobReceipt(s.jobs![0])));
server.handlers.set('job/cancel', () => {
  s.jobs![0] = { ...s.jobs![0], state: 'cancelled', result: { status: { type: 'cancelled', reason: 'user_requested', phase: 'during_execution' }, duration_ms: "1", content: [{ type: 'text', text: 'Process settled' }] } };
  void server.update('A', structuredClone(s));
  return { type: 'job_cancellation_accepted', job_id: s.jobs![0].job_id };
});
server.handlers.set('artifact/read', () => ({ type: 'artifact_bytes', data: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aZ1sAAAAASUVORK5CYII=' }));
await server.attached('A');
const artifacts = new ArtifactResources(server.client, 'A');
declare global { interface Window { activityFixture: { jobs(count: number): Promise<void>; settle(id: string): Promise<void> } } }
window.activityFixture = {
  async jobs(count) { s.jobs = Array.from({ length: count }, (_, i): RuntimeClientJob => ({ job_id: `job-${i}`, tool_id: 'tool-bash', tool_name: `Task ${i + 1}`, state: 'running', bash: { command: `printf 'task ${i + 1}'`, description: `Background task ${i + 1}` } })); await server.update('A', structuredClone(s)); },
  async settle(id) { s.jobs = s.jobs!.map(job => job.job_id === id ? { ...job, state: 'succeeded', result: { status: { type: 'success' }, duration_ms: '2500', content: [{ type: 'text', text: `Final output ${job.job_id}` }] } } : job); await server.update('A', structuredClone(s)); },
};
function Fixture() {
  const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
  const backgroundChat = <><section aria-label="Background launch in chat"><Tool tool={{ committed_result_deferred: false, occurrence: { locator: { type: 'by_message_id', message_id: 'background-launch' }, block_index: 0 }, assembly_complete: true, message_id: 'background-launch', block_index: 0, call_id: 'launch-call', tool_id: 'tool-bash', name: 'bash', state: { type: 'settled', arguments: '{"command":"sleep 5","execution_mode":"background"}', result: { status: { type: 'success' }, duration_ms: '0', content: [{ type: 'json', value: { job_id: 'job-launched', tool: 'bash', state: 'starting', output_path: '/workspace/output.txt' } }] } } }}/></section>
    <section aria-label="Background completion in chat"><InboundMessage message={{ role: 'user', id: 'background-job-finished-terminal', source: 'runtime', kind: 'message', content: [{ type: 'text', text: 'Background execution job-finished (bash) settled: failed\n\nResult:\nBuild output failed verification' }] }}/></section></>;
  return <div style={{ maxWidth: 760, margin: '24px auto', padding: 16 }}><header style={{ position: 'sticky', top: 0, zIndex: 10, display: 'flex', alignItems: 'center', gap: 16, background: 'var(--dsw-alias-bg-base)' }}><h1 style={{ fontSize: 14, minWidth: 0, flex: 1 }}>Jobs and Agents</h1><BackgroundJobs client={server.client} sessionId="A"/><button type="button">Session actions</button></header><div id="subagent-header-actions"/>{backgroundChat}<ArtifactContext.Provider value={artifacts}><Tool tool={{committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:"image-message"},block_index:0},assembly_complete:true, message_id: "image-message", block_index: 0, call_id: "read-image", tool_id: "tool-read-image", name: "read_image", state: { type: "settled", arguments: '{"path":"sample.png"}', result: { status: { type: "success" }, duration_ms: "1", content: [{ type: "image", artifact_id: "artifact_1" }] } } }}/></ArtifactContext.Provider><AgentCard agent={state.views.A.snapshot!.agents![0]} client={server.client} sessionId="A"/><RuntimeFacts snapshot={state.views.A.snapshot!}/><div style={{ height: 1600 }} aria-label="Long conversation"/></div>;
}
createRoot(document.getElementById('root')!).render(<Fixture/>);
