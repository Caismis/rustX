import { useState, useSyncExternalStore } from 'react';
import { createRoot } from 'react-dom/client';
import type { RuntimeClientGoalSnapshot, RuntimeClientJob } from '../../../protocol/app-server/v72';
import { useComposerContent } from '../../src/app/agent/useComposerContent';
import { useWorkflowContent } from '../../src/app/agent/useWorkflowContent';
import { WorkflowRunPanel } from '../../src/app/agent/WorkflowRunPanel';
import { BackgroundJobItem } from '../../src/app/agent/BackgroundJobItem';
import { CompactionCheckpoint } from '../../src/app/agent/CompactionCheckpoint';
import { Interactions } from '../../src/app/agent/Interactions';
import { Content } from '../../src/app/agent/Message';
import { Tool } from '../../src/app/agent/Tool';
import { InboundMessage } from '../../src/app/agent/InboundMessage';
import { SubagentContext } from '../../src/app/agent/subagent-context';
import { Artifact, ArtifactContext } from '../../src/app/components/Artifact';
import { ArtifactResources } from '../../src/client/artifacts';
import { PreviewContext, type PreviewIntents } from '../../src/app/components/ArtifactPreview';
import { nativeTool } from '../native-tool-cases';
import { incrementalAgent } from '../incremental-agent-fixture';
import { GoalDock } from '../../src/app/composer/GoalDock';
import { goalDock } from '../../src/bindings/composer-context';
import { applyTheme, readTheme } from '../../src/app/appearance';
import { Server, checkpoint, domainReceipt, interaction, projectionHead, snapshot } from '../fixture';
import { workflowRun } from '../workflow-data';
import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/design-platform.css';
import '../../src/presentation/theme/reset.css';

applyTheme(readTheme());
const controls = new URLSearchParams(location.search).has('controls');
if (controls) document.title = 'Message controls audit';
const server = new Server();
const goal: RuntimeClientGoalSnapshot = { reference: { id: 'goal', revision: '1' }, objective: 'Complete Goal objective', objective_deferred: false, phase: 'active', blocked_reason_deferred: false, autonomous_round_budget: 4, autonomous_rounds_consumed: 1, origin: { kind: 'runtime_control' }, last_round_message_id_deferred: false, fields: [] };
const run = workflowRun();
const nextRun = { ...run, instances: [...run.instances, { ...run.instances[1], node: 'new-node' }] };
const job: RuntimeClientJob = { job_id: 'refresh-job', tool_id: 'tool-bash', tool_name: 'bash', state: 'succeeded', bash: { command: 'printf rustX', description: 'Refresh output' }, result: { status: { type: 'success' }, duration_ms: '2500', content: [{ type: 'text', text: 'Original job output\nPreserve this reading while refreshing.' }] } };
const nextJob = { ...job, result: { ...job.result!, content: [{ type: 'text' as const, text: 'Updated job output\nThe complete result arrived.' }] } };
const head = { ...projectionHead('Visible summary preview'), summary_preview: { text: 'Visible summary preview', truncated: true } };
const approval = { ...interaction('approval'), body: null, body_complete: false, approval: { tool_id: 'tool-bash', tool_name: 'bash', reason: 'Developer approval required', mode: 'foreground' as const, deferred_fields: [['arguments']] } };
server.snapshots.set('A', { ...snapshot(), goal: { current: goal }, workflows: { revision: '1', runs: [run], omitted_runs: 0 }, jobs: [job], pending_interactions: [controls ? { ...interaction('questionnaire'), body: null, body_complete: false } : approval] });
server.handlers.set('session/checkpoint', () => {
  const value = checkpoint(server.snapshots.get('A')!); value.read_cut.domain_revisions.pending_interactions = '1';
  return { type: 'checkpoint', checkpoint: value };
});
server.handlers.set('job/status', () => domainReceipt('jobs.refresh-job', [nextJob]));
server.handlers.set('conversation/checkpointPage', request => {
  if (request.method !== 'conversation/checkpointPage') throw Error('Expected Workflow read');
  return { type: 'checkpoint_page', page: { read_cut: request.params.cut, cursor: request.params.cut.revision, domain: 'workflows.runs', total: '1', next_offset: null, collections: [], items: [{ offset: '0', identity: 'run', value: nextRun as never, content: [], deferred: false }] } };
});
server.handlers.set('conversation/checkpointValueRead', request => {
  if (request.method !== 'conversation/checkpointValueRead') throw Error('Expected content read');
  const text = request.params.domain === 'goal' ? 'New authoritative Goal objective'
    : request.params.domain === 'jobs.refresh-job' ? JSON.stringify(nextJob) : JSON.stringify({ command: 'printf rustX' });
  return { type: 'checkpoint_value_chunk', chunk: { cut: request.params.cut, domain: request.params.domain, item_offset: 0, path: request.params.path, offset: 0, readable_end: text.length, next_offset: null, representation: request.params.domain === 'goal' ? 'utf8_text' : 'canonical_json_v1', bytes_base64: btoa(text) } };
});
server.handlers.set('session/projectionDetail', () => ({ type: 'projection_detail', detail: { read_cut: checkpoint(server.snapshots.get('A')!).read_cut, checkpoint: { ...head, summary_preview: { text: 'Complete summary ready', truncated: false } }, items: [], next_item: null } }));
await server.attached('A');
if (controls) server.held.add('conversation/checkpointPage');
server.handlers.set('artifact/read', () => ({ type: 'artifact_bytes', data: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aZ1sAAAAASUVORK5CYII=' }));
server.held.add('session/projectionDetail');
let refreshStart = 0;
Object.assign(window, { refreshAudit: {
  refresh: () => {
    refreshStart = server.requests.length;
    for (const method of ['conversation/checkpointPage', 'conversation/checkpointValueRead', 'job/status'] as const) server.held.add(method);
    const next = { ...server.snapshots.get('A')!, goal: { current: { ...goal, reference: { ...goal.reference, revision: '2' }, objective: 'preview', objective_deferred: true } }, workflows: { revision: '2', runs: [{ ...nextRun, instances: nextRun.instances.slice(0, 1) }], omitted_runs: 0 }, jobs: [{ ...nextJob, result: undefined, deferred_fields: [['result']] }] };
    server.snapshots.set('A', next);
    const check = checkpoint(next, '1');
    check.read_cut.domain_revisions.pending_interactions = '2';
    check.collections = [{ domain: 'workflows.runs', revision: '2', total: '1', loaded: '1', next_offset: null }, { domain: 'workflows.runs.@0.instances', revision: '2', total: String(nextRun.instances.length), loaded: '1', next_offset: '1' }];
    server.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: server.target('A'), cursor: '1', event: { type: 'checkpoint_advanced', transition: 'refresh-audit', checkpoint: check } } });
  },
  pending: () => server.requests.length - refreshStart,
  finish: () => {
    for (const method of ['conversation/checkpointPage', 'conversation/checkpointValueRead', 'job/status'] as const) server.held.delete(method);
    for (const row of server.requests.slice(refreshStart)) server.reply(row.request);
    const summary = server.requests.find(row => row.request.method === 'session/projectionDetail');
    if (!summary) throw Error('Expected summary read');
    server.reply(summary.request);
  },
} });

function Audit() {
  const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot), view = state.views.A;
  const content = useComposerContent(server.client, view), workflows = useWorkflowContent(server.client, view, true);
  const [expanded, setExpanded] = useState(true);
  return <main data-refresh-audit style={{ width: 'min(740px, 100%)', margin: '24px auto', padding: '0 16px', boxSizing: 'border-box', display: 'grid', gap: 16 }}>
    <Content messageId="streaming-proposal" streaming blocks={[{ type: 'tool_call', id: 'proposal', tool_id: 'tool-bash', name: 'bash', arguments: {} }]}/>
    <GoalDock state={goalDock(content.snapshot)} observation={view.snapshot?.goal ?? undefined} disabled={false} mutate={async () => ({ status: 'obsolete' })}/>
    <WorkflowRunPanel run={workflows.snapshot!.runs[0]}/>
    <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}><BackgroundJobItem job={view.snapshot!.jobs![0]} now={0} expanded={expanded} onToggle={() => setExpanded(value => !value)} armed={false} onArm={() => {}} stopping={false} onStopped={() => {}} client={server.client} sessionId="A"/></ul>
    <CompactionCheckpoint head={head} client={server.client} sessionId="A"/>
    <Interactions client={server.client} view={view} state={state}/>
  </main>;
}
const events = { previews: [] as string[], downloads: [] as string[] };
const intents: PreviewIntents = { openPreview: artifact => { events.previews.push(artifact.name); }, download: artifact => { events.downloads.push(artifact.name); } };
Object.assign(window, { controlsAudit: { events, reads: () => server.requests.filter(row => row.request.method === 'artifact/read').length } });
const artifacts = controls ? new ArtifactResources(server.client, 'A') : undefined;
function ControlsAudit() {
  const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot), agent = incrementalAgent();
  const launch = nativeTool('tool-bash', 'bash', { command: 'sleep 5', execution_mode: 'background' }, [{ type: 'json', value: { job_id: 'job-controls', tool: 'bash', state: 'starting', output_path: '/workspace/output.txt' } }]);
  return <SubagentContext.Provider value={{ client: server.client, agents: [agent], metrics: {}, metricErrors: {}, opened: [], open: () => {}, close: () => {} }}>
    <ArtifactContext.Provider value={artifacts}><PreviewContext.Provider value={intents}>
      <main data-controls-audit style={{ width: 'min(740px, 100%)', margin: '24px auto', padding: '0 16px', boxSizing: 'border-box', display: 'grid', gap: 16 }}>
        <WorkflowRunPanel run={run}/>
        <Tool tool={launch}/>
        <InboundMessage message={{ role: 'user', id: 'agent-controls', source: { agent: { agent_id: agent.agent_id } }, content: [{ type: 'text', text: 'Recorded child result' }] }}/>
        <Artifact id="file-controls" name="report.txt" mimeType="text/plain"/>
        <Artifact id="image-controls" name="preview.png" image mimeType="image/png"/>
        <Interactions client={server.client} state={state} view={state.views.A}/>
      </main>
    </PreviewContext.Provider></ArtifactContext.Provider>
  </SubagentContext.Provider>;
}
createRoot(document.getElementById('root')!).render(controls ? <ControlsAudit/> : <Audit/>);
