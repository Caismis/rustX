import { traceRecord, traceTool, toolDetail } from '../trace-fixture';
import { nativeTraceFixture } from '../native-trace-fixture';
import { domainReceipt, jobReceipt } from '../fixture';
// Explicit native-state gates. This fixture is not part of the production bundle.
import { useState, useSyncExternalStore } from 'react';
import { createRoot } from 'react-dom/client';
import { Tool } from '../../src/app/agent/Tool';
import { TrajectoryInspector } from '../../src/app/trajectory/TrajectoryInspector';
import type { TrajectoryFacet } from '../../src/app/trajectory/layout';
import { ArtifactContext } from '../../src/app/components/Artifact';
import { ArtifactResources } from '../../src/client/artifacts';
import { AgentCard } from '../../src/app/components/ActivityCards';
import { RuntimeFacts } from '../runtime-facts';
import { SessionPanel } from '../../src/app/agent/SessionPanel';
import { SessionPanelToggle } from '../../src/app/agent/SessionPanelToggle';
import { SessionPanelProvider } from '../../src/app/agent/session-panel-state';
import { InboundMessage } from '../../src/app/agent/InboundMessage';
import type { RuntimeClientAgent, RuntimeClientJob } from '../../../protocol/app-server/v81';
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
const liveOutputs = new Map<string, { text: string; truncated: boolean }>();
server.handlers.set('job/output', request => {
  if (request.method !== 'job/output') throw new Error('Wrong operation');
  const current = liveOutputs.get(request.params.job_id);
  return { type: 'job_output', text: current?.text ?? '', truncated: current?.truncated ?? false };
});
server.handlers.set('job/status', () => (domainReceipt('jobs.'+(s.jobs![0]).job_id,[s.jobs![0]])));
server.handlers.set('job/wait', () => (jobReceipt(s.jobs![0])));
server.handlers.set('job/cancel', () => {
  s.jobs![0] = { ...s.jobs![0], state: 'cancelled', result: { status: { type: 'cancelled', reason: 'user_requested', phase: 'during_execution' }, duration_ms: "1", content: [{ type: 'text', text: 'Process settled' }] } };
  void server.update('A', structuredClone(s));
  return { type: 'job_cancellation_accepted', job_id: s.jobs![0].job_id };
});
const canvas = document.createElement('canvas');
canvas.width = 320;
canvas.height = 180;
const picture = canvas.getContext('2d')!;
picture.fillStyle = '#eef2ff';
picture.fillRect(0, 0, 320, 180);
picture.fillStyle = '#4f67c8';
picture.fillRect(50, 50, 220, 80);
const imageBytes = canvas.toDataURL('image/png').split(',')[1]!;
server.handlers.set('artifact/read', () => ({ type: 'artifact_bytes', data: imageBytes }));
await server.attached('A');
const artifacts = new ArtifactResources(server.client, 'A');
const imagePath = '/workspace/runs/sample.png';
const imageReceipt = 'Read static PNG image';
function ImageTrace() {
  const [facet, setFacet] = useState<TrajectoryFacet>('overview');
  const [fixture] = useState(() => {
    const record = traceTool(295);
    Object.assign(record.tool!, { name: 'read_image', tool_id: 'tool-read-image', outcome: 'success' });
    record.attachments = [{ artifact_id: 'artifact_1', image: true }];
    const detail = toolDetail(295);
    detail.tool!.definition = null;
    detail.tool!.arguments = { value: { arguments: { path: imagePath } }, truncated: false };
    detail.tool!.result!.blocks = [
      { type: 'text', text: { text: imageReceipt, truncated: false } },
      { type: 'image', artifact: record.attachments[0]! },
    ];
    return { record, ...nativeTraceFixture(record, detail) };
  });
  const { record, page, reader } = fixture;
  return <ArtifactContext.Provider value={artifacts}>
    <TrajectoryInspector item={{ type: 'RecordRow', record, display_key: 'image', owner_record_id: record.id, facet: 'overview', label: 'Tool', preview: '' }}
      facet={facet} onFacet={setFacet} detailPage={page} detailReader={reader}
      onLoadDetail={() => {}} onClose={() => {}} records={[record]} executions={new Map()} requestNumbers={new Map()} completeHistory onOpen={() => {}}/>
  </ArtifactContext.Provider>;
}
declare global { interface Window { activityFixture: { jobs(count: number): Promise<void>; settle(id: string): Promise<void>; output(id: string, text: string, truncated?: boolean): void } } }
window.activityFixture = {
  async jobs(count) { s.jobs = Array.from({ length: count }, (_, i): RuntimeClientJob => ({ job_id: `job-${i}`, tool_id: 'tool-bash', tool_name: `Task ${i + 1}`, state: 'running', bash: { command: `printf 'task ${i + 1}'`, description: `Background task ${i + 1}` } })); await server.update('A', structuredClone(s)); },
  async settle(id) { s.jobs = s.jobs!.map(job => job.job_id === id ? { ...job, state: 'succeeded', result: { status: { type: 'success' }, duration_ms: '2500', content: [{ type: 'text', text: `Final output ${job.job_id}` }] } } : job); await server.update('A', structuredClone(s)); },
  output(id, text, truncated = false) { liveOutputs.set(id, { text, truncated }); },
};
function Fixture() {
  const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
  const backgroundChat = <><section aria-label="Background launch in chat"><Tool tool={{ committed_result_deferred: false, occurrence: { locator: { type: 'by_message_id', message_id: 'background-launch' }, block_index: 0 }, assembly_complete: true, message_id: 'background-launch', block_index: 0, call_id: 'launch-call', tool_id: 'tool-bash', name: 'bash', state: { type: 'settled', arguments: '{"arguments":{"command":"sleep 5"},"execution_mode":"background"}', result: { status: { type: 'success' }, duration_ms: '0', content: [{ type: 'json', value: { job_id: 'job-launched', tool: 'bash', state: 'starting', output_path: '/workspace/output.txt' } }] } } }}/></section>
    <section aria-label="Background completion in chat"><InboundMessage message={{ role: 'user', id: 'background-job-finished-terminal', source: 'runtime', kind: 'message', content: [{ type: 'text', text: 'Background execution job-finished (bash) settled: failed\n\nResult:\nBuild output failed verification' }] }}/></section></>;
  return <SessionPanelProvider><div style={{ display: 'flex', flexDirection: 'column', height: '100dvh' }}><header style={{ position: 'sticky', top: 0, zIndex: 10, display: 'flex', alignItems: 'center', gap: 16, maxWidth: 760, margin: '24px auto 0', padding: '0 16px', background: 'var(--dsw-alias-bg-base)' }}><h1 style={{ fontSize: 14, minWidth: 0, flex: 1 }}>Jobs and Agents</h1><SessionPanelToggle client={server.client} sessionId="A"/><button type="button">Session actions</button></header><div style={{ position: 'relative', flex: 1, minHeight: 0, ['--dsh-chat-content-width' as string]: 'min(760px, calc(100% - 32px))' }}><SessionPanel client={server.client} sessionId="A"/><div data-fixture-transcript="" style={{ maxWidth: 760, margin: '0 auto', height: '100%', overflowY: 'auto', boxSizing: 'border-box', padding: 16 }}><div id="subagent-header-actions"/>{backgroundChat}<ArtifactContext.Provider value={artifacts}><Tool tool={{committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:"image-message"},block_index:0},assembly_complete:true, message_id: "image-message", block_index: 0, call_id: "read-image", tool_id: "tool-read-image", name: "read_image", state: { type: "settled", arguments: JSON.stringify({ arguments: { path: imagePath } }), result: { status: { type: "success" }, duration_ms: "1", content: [{ type: 'text', text: imageReceipt }, { type: "image", artifact_id: "artifact_1" }] } } }}/></ArtifactContext.Provider><AgentCard agent={state.views.A.snapshot!.agents![0]} client={server.client} sessionId="A"/><RuntimeFacts snapshot={state.views.A.snapshot!}/><div style={{ height: 1600 }} aria-label="Long conversation"/></div></div></div></SessionPanelProvider>;
}
createRoot(document.getElementById('root')!).render(new URLSearchParams(location.search).has('image-trace') ? <ImageTrace/> : <Fixture/>);
