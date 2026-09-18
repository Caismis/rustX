import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import type { CatalogModelView, ForegroundToolExecution, RuntimeClientSnapshot } from '../../protocol/app-server/v6';
import { AgentControls } from '../src/app/agent/AgentControls';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { Interactions } from '../src/app/agent/Interactions';
import { Tool } from '../src/app/agent/Tool';
import { RpcFailure } from '../src/client/app-server';
import { toolCard } from '../src/bindings/tools';
import { cfg3Effective, cfg3Source } from './cfg3-fixture';
import { Server, snapshot, interaction } from './fixture';
let server: Server;
beforeEach(() => { server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); });
const count = (method: string) => server.requests.filter(row => row.request.method === method).length;
function Control({ kind = 'model' }: { kind?: 'model' | 'permission' }) {
 const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
 return <AgentControls client={server.client} view={state.views.A} kind={kind}/>;
}
const running = (): RuntimeClientSnapshot => ({ ...snapshot(), attempt: { attempt_id: 'native-attempt', phase: { type: 'running' }, turn: 1, execution_settings: { resource_revision: '1', approval_mode: 'policy' } } });
function modelFixture() {
 const model = cfg3Effective().effective_model;
 model.configured.model = model.effective.model = 'exact/model';
 model.effective.reasoningProfile = 'deliberate';
 const catalog: CatalogModelView[] = ['exact/model', 'other'].map(id => ({ model: id, protocol: 'openai_responses', contextWindow: 128000, maxOutputTokens: 8192, declaredCapabilities: model.effective.declaredCapabilities, effectiveCapabilities: model.effective.capabilities, credentialSource: { type: 'literal' }, reasoningProfiles: id === 'other' ? [] : [{ id: 'deliberate', enabled: true }, { id: 'brief', enabled: true }], defaultReasoningProfile: id === 'other' ? null : 'deliberate' }));
 server.snapshots.set('A', { ...snapshot(), model });
 server.handlers.set('settings/models', () => ({ type: 'models', catalog: { models: catalog } }));
 server.handlers.set('settings/model', () => ({ type: 'model', model: server.snapshots.get('A')!.model! }));
 server.handlers.set('settings/setModel', request => {
   if (request.method !== 'settings/setModel') throw new Error('wrong request');
   const next = structuredClone(server.snapshots.get('A')!);
   next.model!.configured = request.params.config;
   next.model!.effective.model = request.params.config.model;
   next.model!.effective.reasoningProfile = request.params.config.reasoningProfile;
   server.snapshots.set('A', next);
   return { type: 'model', model: next.model! };
 });
 return catalog;
}
async function openModels() {
 await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Model and reasoning' })));
 await waitFor(() => expect(screen.queryByText('Reading native models…')).toBeNull());
}
it('model/profile menu advertises only exact native values and acknowledgement alone never changes selection', async () => {
 modelFixture(); await server.attached('A'); render(<Control/>); await openModels();
 expect(count('settings/models')).toBe(1); expect(count('settings/model')).toBe(1);
 fireEvent.click(screen.getByRole('menuitem', { name: 'Reasoning profile' }));
 expect(screen.getByRole('menuitem', { name: 'brief' })).toBeTruthy();
 expect(screen.queryByText('high')).toBeNull(); expect(screen.queryByText('off')).toBeNull();
 server.held.add('settings/setModel'); server.held.add('session/snapshot');
 await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'brief' })));
 const request = await server.waitFor('settings/setModel', 1);
 expect(request.params).toEqual({ target: server.target('A'), config: { model: 'exact/model', reasoningProfile: 'brief' } });
 await act(async () => server.reply(request));
 expect(screen.getByRole('button', { name: 'Model and reasoning' }).textContent).toContain('deliberate');
 expect(count('settings/setModel')).toBe(1);
 expect(server.client.getSnapshot().views.A.modelMutation?.status).toBe('acknowledged');
 await expect(server.client.send('A', 'dependent turn')).rejects.toThrow('Reread native model state');
 expect(count('turn/start')).toBe(0);
 const read = await server.waitFor('session/snapshot', 2);
 await act(async () => server.reply(read));
 expect(screen.getByRole('button', { name: 'Model and reasoning' }).textContent).toContain('brief');
 expect(server.client.getSnapshot().views.A.modelMutation).toBeUndefined();
});
it('lost model mutation is visible uncertainty; reconnect invalidates catalog and never replays', async () => {
 const catalog = modelFixture(); await server.attached('A'); render(<Control/>); await openModels();
 fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
 server.held.add('settings/setModel');
 await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'other' })));
 const request = await server.waitFor('settings/setModel', 1);
 await act(async () => { server.commit(request); server.socket.close(); });
 expect(server.client.getSnapshot().uncertain).toHaveLength(1);
 expect(server.client.getSnapshot().views.A.modelMutation?.status).toBe('uncertain');
 expect(count('settings/setModel')).toBe(1);
 catalog.splice(0, 1);
 await act(async () => server.connect());
 expect(count('settings/setModel')).toBe(1);
 await openModels();
 expect(count('settings/models')).toBe(2);
 fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
 expect(screen.queryByRole('menuitem', { name: 'exact/model' })).toBeNull();
});
it('permission save uses exact source CAS and distinguishes desired, published, and frozen active policy', async () => {
 const source = cfg3Source(); source.prospective_approval_mode = 'policy';
 server.snapshots.set('A', running());
 server.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: structuredClone(source), session_revision: '1' }));
 server.handlers.set('configuration/sourceWrite', request => {
   if (request.method !== 'configuration/sourceWrite') throw new Error('wrong request');
   expect(request.params.expected_revision).toBe('workspace-1');
   expect(request.params.mutation).toEqual({ kind: 'config', scope: 'workspace', mutation: { unit: 'approval', authored: 'full_access' } });
   source.prospective_approval_mode = 'full_access'; source.workspace.revision = 'workspace-2'; source.loaded!.pending_reload = true;
   return { type: 'source_settings', projection: structuredClone(source), session_revision: '1' };
 });
 await server.attached('A'); render(<Control kind="permission"/>);
 await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Approval mode' })));
 await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'Full access' })));
 expect(count('configuration/sourceWrite')).toBe(1); expect(count('configuration/reload')).toBe(0);
 expect(screen.getByText('Effective for running attempt: policy')).toBeTruthy();
 expect(screen.getByText('Desired: full_access · pending Reload')).toBeTruthy();
 expect((screen.getByRole('button', { name: 'Apply saved policy' }) as HTMLButtonElement).disabled).toBe(true);
});
it('one Stop gesture issues one request, and only native snapshot settlement releases the cancellation guard', async () => {
 server.snapshots.set('A', running()); await server.attached('A'); server.held.add('turn/cancel');
 const stopped = server.client.cancelTurn('A'); await server.client.cancelTurn('A');
 const request = await server.waitFor('turn/cancel', 1);
 expect(count('turn/cancel')).toBe(1);
 server.reply(request); await stopped;
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
 expect(server.client.getSnapshot().views.A.cancellation?.status).toBe('acknowledged');
 await server.client.cancelTurn('A'); expect(count('turn/cancel')).toBe(1);
 const settled = running(); settled.attempt!.phase = { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } };
 await server.update('A', settled);
 expect(server.client.getSnapshot().views.A.cancellation).toBeUndefined();
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('settled');
 server.socket.close(); expect(count('turn/cancel')).toBe(1);
});
it('pending approval restores after browser absence; accepted response cannot settle a still-pending snapshot', async () => {
 await server.attached('A'); server.socket.close();
 server.snapshots.get('A')!.pending_interactions = [interaction('approval')]; await server.connect();
 function Pending() { const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot); return <Interactions client={server.client} state={state} view={state.views.A} run={work => { void work().catch(() => {}); }}/>; }
 render(<Pending/>); server.held.add('interaction/respond');
 await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Allow once' })); fireEvent.click(screen.getByRole('button', { name: 'Allow once' })); });
 const request = await server.waitFor('interaction/respond', 1);
 server.handlers.set('interaction/respond', () => ({ type: 'interaction_settled', interaction: interaction('approval').interaction }));
 await act(async () => server.reply(request));
 expect(count('interaction/respond')).toBe(1);
 expect((screen.getByRole('button', { name: 'Allow once' }) as HTMLButtonElement).disabled).toBe(true);
 await act(async () => server.update('A', snapshot()));
 expect(screen.queryByRole('button', { name: 'Allow once' })).toBeNull();
});
it('Tool identity selects presentation; running, terminal failures, cancellation and uncertainty stay native', () => {
 const tool: ForegroundToolExecution = { call_id: 'call', tool_id: 'tool-bash', name: 'shell', state: { type: 'running', arguments: '{"command":"pwd"}' } };
 const ui = render(<Tool tool={tool}/>);
 expect(ui.container.querySelector('[data-tool-renderer="bash"]')).toBeTruthy();
 expect(screen.getByLabelText('Tool status').textContent).toBe('running');
 for (const [status, expected] of [[{ type: 'success' }, 'success'], [{ type: 'failed', error: 'native failure' }, 'failure'], [{ type: 'cancelled', reason: 'user_requested', phase: 'during_execution' }, 'cancelled'], [{ type: 'outcome_unknown', detail: 'not proven' }, 'uncertain']] as const) {
   tool.state = { type: 'settled', arguments: '{}', result: { status, duration_ms: 1 } };
   ui.rerender(<Tool tool={{ ...tool }}/>); expect(screen.getByLabelText('Tool status').textContent).toBe(expected);
 }
 tool.tool_id = 'mcp.read'; tool.name = 'read';
 expect(toolCard(tool).variant).toBe('generic');
 for (const [id, variant] of [['tool-read', 'read'], ['tool-write', 'write'], ['tool-edit', 'edit'], ['tool-glob', 'search'], ['tool-grep', 'search']]) expect(toolCard({ ...tool, tool_id: id }).variant).toBe(variant);
});
it('reasoning and Tool rows remain at native canonical positions while live state changes', () => {
 const s = running();
 const call = { id: 'call', tool_id: 'tool-bash', name: 'bash', arguments: { command: 'pwd' } };
 const tool: ForegroundToolExecution = { call_id: call.id, tool_id: call.tool_id, name: call.name, state: { type: 'assembled', arguments: '{"command":"pwd"}' } };
 s.transcript.entries = [{ cursor: '10', tool_calls: [tool], item: { type: 'message', message: { role: 'assistant', id: 'a', content: [{ type: 'reasoning', text: 'First reason' }, { type: 'tool_call', ...call }, { type: 'text', text: 'After call' }] } } }];
 s.attempt!.foreground = [{ ...tool, state: { type: 'running', arguments: tool.state.arguments } }];
 const ui = render(<AgentTranscript snapshot={s}/>);
 expect(ui.container.textContent).toMatch(/First reason.*bash.*running.*After call/s);
 expect(ui.container.querySelectorAll('[data-tool-call-id]')).toHaveLength(1);
});

it('an acknowledged Stop remains fenced when its authoritative reread fails', async () => {
 server.snapshots.set('A', running()); await server.attached('A');
 server.handlers.set('session/snapshot', () => { throw new RpcFailure({ code: -32000, message: 'Read unavailable' }); });
 await expect(server.client.cancelTurn('A')).rejects.toThrow('Read unavailable');
 expect(server.client.getSnapshot().views.A.cancellation?.status).toBe('acknowledged');
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
 server.handlers.delete('session/snapshot'); await server.client.refresh('A');
 await server.client.cancelTurn('A'); expect(count('turn/cancel')).toBe(1);
});
