import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import type { CatalogModelView, ForegroundToolExecution, RuntimeClientSnapshot } from '../../protocol/app-server/v19';
import { AgentControls } from '../src/app/agent/AgentControls';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { Interactions } from '../src/app/agent/Interactions';
import { Tool } from '../src/app/agent/Tool';
import { RpcFailure } from '../src/client/app-server';
import { toolCard } from '../src/bindings/tools';
import { cfg3Effective } from './cfg3-data';
import { Server, snapshot, interaction } from './fixture';
let server: Server;
beforeEach(() => { server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); });
const count = (method: string) => server.requests.filter(row => row.request.method === method).length;
function Control() {
 const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
 return <AgentControls client={server.client} view={state.views.A}/>;
}
const running = (): RuntimeClientSnapshot => ({ ...snapshot(), attempt: { attempt_id: 'native-attempt', phase: { type: 'running' }, turn: 1, execution_settings: { resource_revision: '1', approval_mode: 'policy' } } });
function modelFixture() {
 const model = cfg3Effective().effective_model;
 model.configured.model = model.effective.model = 'exact/model';
 model.effective.reasoningProfile = 'deliberate';
 const catalog: CatalogModelView[] = ['exact/model', 'other'].map(id => ({ model: id, protocol: 'openai_responses', contextWindow: 128000, maxOutputTokens: 8192, declaredCapabilities: model.effective.declaredCapabilities, effectiveCapabilities: model.effective.capabilities, credentialSource: { type: 'literal' }, reasoningProfiles: id === 'other' ? [] : [{ id: 'deliberate', enabled: true }, { id: 'brief', enabled: true }], defaultReasoningProfile: id === 'other' ? null : 'deliberate' }));
 server.snapshots.set('A', { ...snapshot(), model });
 server.handlers.set('session/models', () => ({ type: 'models', catalog: { models: catalog } }));
 server.handlers.set('session/model', () => ({ type: 'model', model: server.snapshots.get('A')!.model! }));
 server.handlers.set('session/setModel', request => {
   if (request.method !== 'session/setModel') throw new Error('wrong request');
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
 expect(count('session/models')).toBe(1); expect(count('session/model')).toBe(1);
 fireEvent.click(screen.getByRole('menuitem', { name: 'Reasoning profile' }));
 expect(screen.getByRole('menuitem', { name: 'brief' })).toBeTruthy();
 expect(screen.queryByText('high')).toBeNull(); expect(screen.queryByText('off')).toBeNull();
 server.held.add('session/setModel'); server.held.add('session/snapshot');
 await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'brief' })));
 const request = await server.waitFor('session/setModel', 1);
 expect(request.params).toEqual({ target: server.target('A'), config: { model: 'exact/model', reasoningProfile: 'brief' } });
 await act(async () => server.reply(request));
 expect(screen.getByRole('button', { name: 'Model and reasoning' }).textContent).toContain('deliberate');
 expect(count('session/setModel')).toBe(1);
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
 server.held.add('session/setModel');
 await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'other' })));
 const request = await server.waitFor('session/setModel', 1);
 await act(async () => { server.commit(request); server.socket.close(); });
 expect(server.client.getSnapshot().uncertain).toHaveLength(1);
 expect(server.client.getSnapshot().views.A.modelMutation?.status).toBe('uncertain');
 expect(count('session/setModel')).toBe(1);
 catalog.splice(0, 1);
 await act(async () => server.connect());
 expect(count('session/setModel')).toBe(1);
 await openModels();
 expect(count('session/models')).toBe(2);
 fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
 expect(screen.queryByRole('menuitem', { name: 'exact/model' })).toBeNull();
});
it('Session controls never expose source-authoring permission controls', async () => {
 modelFixture(); await server.attached('A'); render(<Control/>); await openModels();
 expect(screen.queryByRole('button', { name: 'Approval mode' })).toBeNull();
 expect(count('configuration/sourcesRead')).toBe(0); expect(count('configuration/sourceWrite')).toBe(0);
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
 const tool: ForegroundToolExecution = { message_id: 'a', block_index: 0, call_id: 'call', tool_id: 'tool-bash', name: 'shell', state: { type: 'running', arguments: '{"command":"pwd"}' } };
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
 const tool: ForegroundToolExecution = { message_id: 'a', block_index: 1, call_id: call.id, tool_id: call.tool_id, name: call.name, state: { type: 'assembled', arguments: '{"command":"pwd"}' } };
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

it('live foreground overlays only its exact canonical occurrence when historical Attempts reuse a provider call ID', () => {
 const s = running();
 const old: ForegroundToolExecution = { message_id: 'assistant-A', block_index: 1, call_id: 'call-1', tool_id: 'tool-bash', name: 'bash', state: { type: 'settled', arguments: '{}', result: { status: { type: 'success' }, duration_ms: 1, content: [{ type: 'text', text: 'old-result' }] } } };
 const current: ForegroundToolExecution = { ...old, message_id: 'assistant-B', state: { type: 'assembled', arguments: '{}' } };
 s.transcript.entries = [old, current].map((tool, index) => ({ cursor: String(index + 1), tool_calls: [tool], item: { type: 'message', message: { role: 'assistant', id: tool.message_id, content: [{ type: 'reasoning', text: `Reason ${index}` }, { type: 'tool_call', id: tool.call_id, tool_id: tool.tool_id, name: tool.name, arguments: {} }] } } }));
 s.attempt!.foreground = [{ ...current, state: { type: 'running', arguments: '{}' } }];
 const ui = render(<AgentTranscript snapshot={s}/>);
 const oldRow = within(ui.container.querySelector('[data-chat-anchor-key="message:assistant-A"]')! as HTMLElement);
 const newRow = within((ui.container.querySelector('[data-chat-anchor-key="message:assistant-B"]')! as HTMLElement));
 expect(newRow.getByLabelText('Tool status').textContent).toBe('running');
 expect((ui.container.querySelector('[data-chat-anchor-key="message:assistant-B"]')! as HTMLElement).querySelectorAll('[data-tool-call-id]')).toHaveLength(1);
 fireEvent.click(oldRow.getByRole('button', { name: /bash/ }));
 expect(oldRow.getByText('old-result')).toBeTruthy();
 expect(newRow.queryByText('old-result')).toBeNull();
 expect(oldRow.getByLabelText('Tool status').textContent).toBe('success');
 // Canonical settlement of this same occurrence wins over lagging live state.
 s.transcript.entries![1].tool_calls = [{ ...current, state: { type: 'settled', arguments: '{}', result: { status: { type: 'success' }, duration_ms: 1, content: [{ type: 'text', text: 'new-result' }] } } }];
 ui.rerender(<AgentTranscript snapshot={{ ...s }}/>);
 expect(newRow.getByLabelText('Tool status').textContent).toBe('success');
 fireEvent.click(newRow.getByRole('button', { name: /bash/ }));
 expect(newRow.getByText('new-result')).toBeTruthy();
 expect(newRow.queryByText('old-result')).toBeNull();
 s.transcript.entries![1].tool_calls = [current];
 // A historical unresolved occurrence also cannot borrow B's live state.
 s.transcript.entries![0].tool_calls = [{ ...old, state: { type: 'assembled', arguments: '{}' } }];
 ui.rerender(<AgentTranscript snapshot={{ ...s }}/>);
 expect(oldRow.getByLabelText('Tool status').textContent).toBe('assembled');
 expect(newRow.getByLabelText('Tool status').textContent).toBe('running');
});
it('native Goal activity specializes outcomes without generic cards and retains exact execution details', () => {
 const tool: ForegroundToolExecution = { message_id: 'a', block_index: 0, call_id: 'goal-call', tool_id: 'native.create_goal', name: 'create_goal', state: { type: 'running', arguments: '{}' } };
 const ui = render(<Tool tool={tool}/>);
 expect(screen.getByText('Starting Goal')).toBeTruthy();
 expect(screen.getByLabelText('Goal activity status').textContent).toBe('running');
 expect(ui.container.querySelector('[data-tool-renderer]')).toBeNull();
 for (const [name, args, label] of [['create_goal', '{}', 'Goal started'], ['update_goal', '{"action":"complete"}', 'Goal completed'], ['update_goal', '{"action":"blocked"}', 'Goal blocked'], ['get_goal', '{}', 'Goal checked']] as const) {
   const execution: ForegroundToolExecution = { ...tool, tool_id: `native.${name}`, name, state: { type: 'settled', arguments: args, result: { status: { type: 'success' }, duration_ms: 0 } } };
   ui.rerender(<Tool tool={execution}/>);
   expect(screen.getByText(label)).toBeTruthy();
   expect(ui.container.querySelector('[data-tool-renderer]')).toBeNull();
   expect(JSON.parse(ui.container.querySelector('pre')!.textContent!)).toEqual(execution);
 }
 for (const status of [{ type: 'failed', error: 'native rejection' }, { type: 'cancelled', reason: 'user_requested', phase: 'during_execution' }, { type: 'outcome_unknown', detail: 'unknown' }] as const) {
   ui.rerender(<Tool tool={{ ...tool, state: { type: 'settled', arguments: '{}', result: { status, duration_ms: 0 } } }}/>);
   expect(screen.queryByText('Goal started')).toBeNull();
   expect(screen.getByLabelText('Goal activity status').textContent).toBe(status.type.replaceAll('_', ' '));
 }
 ui.rerender(<Tool tool={{ ...tool, tool_id: 'mcp.create_goal' }}/>);
 expect(ui.container.querySelector('[data-goal-activity]')).toBeNull();
 expect(ui.container.querySelector('[data-tool-renderer="generic"]')).toBeTruthy();
});
