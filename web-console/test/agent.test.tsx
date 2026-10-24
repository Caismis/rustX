import { domainReceipt } from './fixture';
import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import type { CatalogModelView, ForegroundToolExecution, RuntimeClientSnapshot } from '../../protocol/app-server/v57';
import { AgentComposer } from '../src/app/agent/AgentComposer';
import { ComposerDraft } from '../src/app/composer/draft';
import { localeController } from '../src/locale/controller';
import { AgentControls } from '../src/app/agent/AgentControls';
import { ModelSelect } from '../src/presentation/agent/ModelSelect';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { Interactions } from '../src/app/agent/Interactions';
import { Tool } from '../src/app/agent/Tool';
import { RpcFailure } from '../src/client/errors';
import { toolCard } from '../src/bindings/tools';
import { cfg3Effective, cfg3Source } from './cfg3-data';
import { Server, snapshot, interaction } from './fixture';
let server: Server;
beforeEach(() => { server = new Server(); server.handlers.set('turn/cancel', request => ({type:'cancellation_accepted',attempt_id:request.method==='turn/cancel' ? server.snapshots.get(request.params.target.session_id)?.attempt?.attempt_id ?? 'attempt-A' : 'attempt-A'})); });
afterEach(() => { cleanup(); server.client.disconnect(); act(() => localeController.setLocale('en')); });
const count = (method: string) => server.requests.filter(row => row.request.method === method).length;
it('keeps initial model loading inside the popup and cached refresh out of the trigger layout', () => {
 const props = { current: 'exact/model', profile: 'brief', disabled: false, loading: true, load: () => {}, choose: async () => true, initialOpen: true };
 const { rerender } = render(<ModelSelect {...props} choices={[]} />);
 expect(screen.getByText('Reading native models…').closest('[role="menu"]')).not.toBeNull();
 const trigger = screen.getByRole('button', { name: 'Model and thinking effort' });
 expect(trigger.querySelector('[role="status"]')).toBeNull();
 expect(screen.getByRole('status').closest('[role="menu"]')).not.toBeNull();
 rerender(<ModelSelect {...props} choices={[{ id: 'exact/model', profiles: [{ id: 'brief', label: 'brief' }] }]} />);
 expect(screen.queryByText('Reading native models…')).toBeNull();
 expect(screen.getByRole('menu').getAttribute('aria-busy')).toBe('true');
 expect(screen.getByRole('button', { name: 'Model and thinking effort' })).toBe(trigger);
 expect(trigger.textContent).toContain('exact/model');
 rerender(<ModelSelect {...props} loading={false} choices={[{ id: 'exact/model', profiles: [{ id: 'brief', label: 'brief' }] }]} />);
 expect(screen.queryByText('Reading native models…')).toBeNull();
});
function Control() {
 const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
 return <AgentControls client={server.client} view={state.views.A}/>;
}
const running = (): RuntimeClientSnapshot => ({ ...snapshot(), attempt: { attempt_id: 'native-attempt', phase: { type: 'running' }, turn: 1, execution_settings: { resource_revision: '1', approval_mode: 'policy' } } });
function modelFixture() {
 const model = cfg3Effective().effective_model;
 model.configured.model = model.effective!.model = 'exact/model';
 model.effective!.profile = 'deliberate';
 const catalog: CatalogModelView[] = ['exact/model', 'other'].map(id => ({ model: id, protocol: 'openai_responses', contextWindow: 128000, maxOutputTokens: 8192, declaredCapabilities: model.effective!.declaredCapabilities, effectiveCapabilities: model.effective!.capabilities, credentialSource: { type: 'literal' }, profiles: id === 'other' ? [] : [{ id: 'deliberate', reasoningEnabled: true }, { id: 'brief', reasoningEnabled: true }], defaultProfile: id === 'other' ? null : 'deliberate' }));
 server.snapshots.set('A', { ...snapshot(), model });
 server.handlers.set('session/models', () => (domainReceipt('model_catalog.models',({ models: catalog }).models??[])));
 server.handlers.set('session/model', () => (domainReceipt('model',[server.snapshots.get('A')!.model!])));
 server.handlers.set('session/setModel', request => {
   if (request.method !== 'session/setModel') throw new Error('wrong request');
   const next = structuredClone(server.snapshots.get('A')!);
   next.model!.configured = request.params.config;
   next.model!.effective!.model = request.params.config.model;
   next.model!.effective!.profile = request.params.config.profile ?? catalog.find(row => row.model === request.params.config.model)?.defaultProfile;
   server.snapshots.set('A', next);
   return {type:'model_set',settings_revision:'1'};
 });
 return catalog;
}
async function openModels() {
 if (screen.getByRole('button', { name: 'Model and thinking effort' }).getAttribute('aria-expanded') !== 'true') await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Model and thinking effort' })));
 if (server.client.getSnapshot().views.A?.modelMutation && screen.queryByRole('button', { name: 'Reread models' })) await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Reread models' })));
 await waitFor(() => expect(screen.getByRole('menu').getAttribute('aria-busy')).toBe('false'));
 if (document.querySelector('[data-model-menu]')?.getAttribute('data-pane') !== 'root') fireEvent.keyDown(screen.getByRole('button', { name: 'Model and thinking effort' }), { key: 'Escape' });
}
it('selects a profile during execution without changing its frozen profile, parameters, output limit or summary', async () => {
 modelFixture();
 const initial = server.snapshots.get('A')!;
 initial.model!.configured = { model: 'exact/model', profile: 'deliberate', requestParams: { temperature: 0.2 }, maxOutputTokens: 777, summaryModel: { mode: 'explicit', model: 'summary/original' } };
 initial.model!.effective!.requestParams = { effort: 'high', temperature: 0.2 };
 initial.model!.effective!.maxOutputTokens = 777;
 const frozen = { primary: structuredClone(initial.model!.effective!), summary: { ...structuredClone(initial.model!.effective!), mode: 'explicit' as const, model: 'summary/original' } };
 initial.attempt = { ...running().attempt!, model: frozen };
 await server.attached('A'); render(<Control/>); await openModels();
 expect((screen.getByRole('button', { name: 'Model and thinking effort' }) as HTMLButtonElement).disabled).toBe(false);
 fireEvent.click(screen.getByRole('menuitem', { name: 'Thinking effort' }));
 server.held.add('session/setModel'); server.held.add('session/checkpoint');
 await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: 'brief' })));
 const set = await server.waitFor('session/setModel', 1);
 expect(set.params).toMatchObject({ config: { ...initial.model!.configured, profile: 'brief' } });
 await act(async () => server.reply(set));
 expect(screen.getByRole('button', { name: 'Model and thinking effort' }).textContent).toContain('deliberate');
 const reread = await server.waitFor('session/checkpoint', 2);
 await act(async () => server.reply(reread));
 expect(screen.getByRole('button', { name: 'Model and thinking effort' }).textContent).toContain('brief');
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.model).toEqual(frozen);
 expect(count('session/setModel')).toBe(1);
 server.held.delete('session/checkpoint');
 await act(async () => { await server.client.release('A'); await server.client.attach('A'); });
 expect(screen.getByRole('button', { name: 'Model and thinking effort' }).textContent).toContain('brief');
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.model).toEqual(frozen);
});
it('does not dim the model trigger during a held catalog read', async () => {
 modelFixture(); await server.attached('A'); server.held.add('session/models'); render(<Control/>);
 const trigger = screen.getByRole('button', { name: 'Model and thinking effort' }) as HTMLButtonElement;
 fireEvent.click(trigger);
 await server.waitFor('session/models', 1);
 expect(trigger.disabled).toBe(false);
 expect(screen.getByText('Reading native models…').closest('[role="menu"]')).not.toBeNull();
 expect(count('session/setModel')).toBe(0);
});
it('model/profile menu advertises only exact native values and acknowledgement alone never changes selection', async () => {
 modelFixture(); await server.attached('A'); render(<Control/>); await openModels();
 expect(count('session/models')).toBe(1); expect(count('session/model')).toBe(0); expect(count('session/checkpoint')).toBe(1);
 fireEvent.click(screen.getByRole('menuitem', { name: 'Thinking effort' }));
 expect(screen.getByRole('menuitemradio', { name: 'brief' })).toBeTruthy();
 expect(screen.queryByText('high')).toBeNull(); expect(screen.queryByText('off')).toBeNull();
 server.held.add('session/setModel'); server.held.add('session/checkpoint');
 await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: 'brief' })));
 const request = await server.waitFor('session/setModel', 1);
 expect(request.params).toEqual({ target: server.target('A'), config: { model: 'exact/model', profile: 'brief' } });
 await act(async () => server.reply(request));
 expect(screen.getByRole('button', { name: 'Model and thinking effort' }).textContent).toContain('deliberate');
 expect(count('session/setModel')).toBe(1);
 expect(server.client.getSnapshot().views.A.modelMutation?.status).toBe('acknowledged');
 await expect(server.client.send('A', 'dependent turn')).rejects.toThrow('Reread native model state');
 expect(count('turn/start')).toBe(0);
 const read = await server.waitFor('session/checkpoint', 2);
 await act(async () => server.reply(read));
 expect(screen.getByRole('button', { name: 'Model and thinking effort' }).textContent).toContain('brief');
 expect(server.client.getSnapshot().views.A.modelMutation).toBeUndefined();
});
it('lost model mutation is visible uncertainty; reconnect invalidates catalog and never replays', async () => {
 const catalog = modelFixture(); await server.attached('A'); render(<Control/>); await openModels();
 fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
 server.held.add('session/setModel');
 await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: 'other' })));
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
 expect(screen.queryByRole('menuitemradio', { name: 'exact/model' })).toBeNull();
});
it('a rejected model selection announces outside the composer and retains the native selection until reread', async () => {
 modelFixture(); await server.attached('A');
 const diagnostic = 'provider other credential environment variable OTHER_KEY is not set';
 server.handlers.set('session/setModel', () => { throw new RpcFailure({ code: -32000, message: diagnostic }); });
 const ui = render(<div data-composer-card><Control/></div>);
 await openModels(); fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
 await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: 'other' })));
 const toast = await screen.findByRole('alert');
 expect(toast.textContent).toContain(diagnostic);
 expect(toast.parentElement).toBe(document.body);
 expect(ui.container.querySelector('[role="alert"]')).toBeNull();
 expect(screen.getByRole('menu')).toBeTruthy();
 expect(screen.getByRole('button', { name: 'Model and thinking effort' }).textContent).toContain('exact/model');
 await expect(server.client.send('A', 'dependent turn')).rejects.toThrow('Reread native model state');
 expect(count('turn/start')).toBe(0);
 await openModels();
 expect(server.client.getSnapshot().views.A.modelMutation).toBeUndefined();
 expect(count('session/setModel')).toBe(1);
 fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
 await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: 'other' })));
 expect(await screen.findByRole('alert')).not.toBe(toast);
 expect(count('session/setModel')).toBe(2);
});
it('catalog read errors remain retryable inside the model menu without a selection toast', async () => {
 modelFixture(); await server.attached('A'); render(<Control/>);
 server.handlers.set('session/models', () => { throw new RpcFailure({ code: -32000, message: 'Catalog read unavailable' }); });
 fireEvent.click(screen.getByRole('button', { name: 'Model and thinking effort' }));
 const error = await screen.findByRole('alert');
 expect(error.textContent).toContain('Catalog read unavailable');
 expect(error.closest('[role="menu"]')).not.toBeNull();
 expect(document.querySelector('[data-toast]')).toBeNull();
 modelFixture();
 await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Reread models' })));
 await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
 expect(count('session/models')).toBe(2);
});
it('a Session replacement retires its model-selection error announcement', async () => {
 modelFixture(); await server.attached('A'); render(<Control/>); await openModels();
 server.handlers.set('session/setModel', () => { throw new RpcFailure({ code: -32000, message: 'Missing provider credential' }); });
 fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
 await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: 'other' })));
 await screen.findByRole('alert');
 await act(async () => server.client.release('A'));
 expect(screen.queryByRole('alert')).toBeNull();
});
it('Session controls never expose source-authoring permission controls', async () => {
 modelFixture(); await server.attached('A'); render(<Control/>); await openModels();
 expect(screen.queryByRole('button', { name: 'Approval mode' })).toBeNull();
 expect(count('configuration/sourcesRead')).toBe(0); expect(count('configuration/sourceWrite')).toBe(0);
});
it('one Stop gesture issues one request, and only native snapshot settlement releases the cancellation guard', async () => {
 server.snapshots.set('A', running()); await server.attached('A'); server.held.add('turn/cancel');
 const expected = server.client.cancellationTarget('A')!;
 const stopped = server.client.cancelTurn(expected); await server.client.cancelTurn(expected);
 const request = await server.waitFor('turn/cancel', 1);
 expect(count('turn/cancel')).toBe(1);
 server.reply(request); await stopped;
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
 expect(server.client.getSnapshot().views.A.cancellation?.status).toBe('acknowledged');
 await server.client.cancelTurn(expected); expect(count('turn/cancel')).toBe(1);
 const settled = running(); settled.attempt!.phase = { type: 'settled', outcome: { type: 'cancelled', reason: 'user_requested' } };
 await server.update('A', settled);
 expect(server.client.getSnapshot().views.A.cancellation).toBeUndefined();
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('settled');
 server.socket.close(); expect(count('turn/cancel')).toBe(1);
});
it('pending approval restores after browser absence; accepted response cannot settle a still-pending snapshot', async () => {
 await server.attached('A'); server.socket.close();
 server.snapshots.get('A')!.pending_interactions = [interaction('approval')]; await server.connect();
 function Pending() { const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot); return <Interactions client={server.client} state={state} view={state.views.A}/>; }
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
 const tool: ForegroundToolExecution = {committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:'a'},block_index:0},assembly_complete:true, message_id: 'a', block_index: 0, call_id: 'call', tool_id: 'tool-bash', name: 'shell', state: { type: 'running', arguments: '{"command":"pwd"}' } };
 const ui = render(<Tool tool={tool}/>);
 expect(ui.container.querySelector('[data-tool-renderer="bash"]')).toBeTruthy();
 expect(screen.getByLabelText('Tool status').textContent).toBe('running');
 for (const [status, expected] of [[{ type: 'success' }, 'success'], [{ type: 'failed', error: 'native failure' }, 'failure'], [{ type: 'cancelled', reason: 'user_requested', phase: 'during_execution' }, 'cancelled'], [{ type: 'outcome_unknown', detail: 'not proven' }, 'uncertain']] as const) {
   tool.state = { type: 'settled', arguments: '{}', result: { status, duration_ms: "1" } };
   ui.rerender(<Tool tool={{ ...tool }}/>); expect(screen.getByLabelText('Tool status').textContent).toBe(expected);
 }
 tool.tool_id = 'mcp.read'; tool.name = 'read';
 expect(toolCard(tool).variant).toBe('generic');
 for (const [id, variant] of [['tool-read', 'read'], ['tool-write', 'write'], ['tool-edit', 'edit'], ['tool-glob', 'search'], ['tool-grep', 'search']]) expect(toolCard({ ...tool, tool_id: id }).variant).toBe(variant);
});
it('reasoning and Tool rows remain at native canonical positions while live state changes', () => {
 const s = running();
 const call = { id: 'call', tool_id: 'tool-bash', name: 'bash', arguments: { command: 'pwd' } };
 const tool: ForegroundToolExecution = {committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:'a'},block_index:1},assembly_complete:true, message_id: 'a', block_index: 1, call_id: call.id, tool_id: call.tool_id, name: call.name, state: { type: 'assembled', arguments: '{"command":"pwd"}' } };
 s.transcript.entries = [{ cursor: '10', tool_calls: [tool], item: { type: 'message', message: { role: 'assistant', id: 'a', content: [{ type: 'reasoning', text: 'First reason' }, { type: 'tool_call', ...call }, { type: 'text', text: 'After call' }] } } }];
 s.attempt!.foreground = [{ ...tool, state: { type: 'running', arguments: tool.state.arguments } }];
 const ui = render(<AgentTranscript snapshot={s}/>);
 expect(ui.container.textContent).toMatch(/First reason.*bash.*running.*After call/s);
 expect(ui.container.querySelectorAll('[data-tool-call-id]')).toHaveLength(1);
});

it('an acknowledged Stop requires no observation reread', async () => {
 server.snapshots.set('A', running()); await server.attached('A');
 server.handlers.set('session/checkpoint', () => { throw new RpcFailure({ code: -32000, message: 'Read unavailable' }); });
 const expected = server.client.cancellationTarget('A')!;
 const reads = count('session/checkpoint');
 await server.client.cancelTurn(expected);
 expect(count('session/checkpoint')).toBe(reads);
 expect(server.client.getSnapshot().views.A.cancellation?.status).toBe('acknowledged');
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
 server.handlers.delete('session/checkpoint'); await server.client.refresh('A');
 await server.client.cancelTurn(expected); expect(count('turn/cancel')).toBe(1);
});

it('live foreground overlays only its exact canonical occurrence when historical Attempts reuse a provider call ID', () => {
 const s = running();
 const old: ForegroundToolExecution = {committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:'assistant-A'},block_index:1},assembly_complete:true, message_id: 'assistant-A', block_index: 1, call_id: 'call-1', tool_id: 'tool-bash', name: 'bash', state: { type: 'settled', arguments: '{}', result: { status: { type: 'success' }, duration_ms: "1", content: [{ type: 'text', text: 'old-result' }] } } };
 const current: ForegroundToolExecution = { ...old, occurrence:{locator:{type:'by_message_id',message_id:'assistant-B'},block_index:1},message_id: 'assistant-B', state: { type: 'assembled', arguments: '{}' } };
 s.transcript.entries = [old, current].map((tool, index) => ({ cursor: String(index + 1), tool_calls: [tool], item: { type: 'message', message: { role: 'assistant', id: tool.message_id!, content: [{ type: 'reasoning', text: `Reason ${index}` }, { type: 'tool_call', id: tool.call_id!, tool_id: tool.tool_id!, name: tool.name, arguments: {} }] } } }));
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
 s.transcript.entries![1].tool_calls = [{ ...current, state: { type: 'settled', arguments: '{}', result: { status: { type: 'success' }, duration_ms: "1", content: [{ type: 'text', text: 'new-result' }] } } }];
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
it('native Goal activity uses shared tool details and retains outcome diagnostics', () => {
 const tool: ForegroundToolExecution = {committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:'a'},block_index:0},assembly_complete:true, message_id: 'a', block_index: 0, call_id: 'goal-call', tool_id: 'native.create_goal', name: 'create_goal', state: { type: 'running', arguments: '{}' } };
 const ui = render(<Tool tool={tool}/>);
 expect(screen.getByText('Starting Goal')).toBeTruthy();
 expect(screen.getByLabelText('Tool status').textContent).toBe('running');
 expect(ui.container.querySelector('[data-tool-renderer="generic"]')).toBeTruthy();
 for (const [name, args, label] of [['create_goal', '{}', 'Goal started'], ['update_goal', '{"action":"complete"}', 'Goal completed'], ['update_goal', '{"action":"blocked"}', 'Goal blocked'], ['get_goal', '{}', 'Goal checked']] as const) {
   const execution: ForegroundToolExecution = { ...tool, tool_id: `native.${name}`, name, state: { type: 'settled', arguments: args, result: { status: { type: 'success' }, duration_ms: "0" } } };
   ui.rerender(<Tool tool={execution}/>);
   expect(screen.getByText(label)).toBeTruthy();
   expect(ui.container.querySelector('[data-tool-renderer="generic"]')).toBeTruthy();
   fireEvent.click(screen.getByRole('button', { expanded: false }));
   expect(ui.container.textContent).not.toContain('\"tool_id\"');
   expect(ui.container.textContent).not.toContain('Execution details');
   fireEvent.click(screen.getByRole('button', { expanded: true }));
 }
 for (const status of [{ type: 'failed', error: 'native rejection' }, { type: 'cancelled', reason: 'user_requested', phase: 'during_execution' }, { type: 'outcome_unknown', detail: 'unknown' }] as const) {
   ui.rerender(<Tool tool={{ ...tool, state: { type: 'settled', arguments: '{}', result: { status, duration_ms: "0" } } }}/>);
   expect(screen.queryByText('Goal started')).toBeNull();
   expect(screen.getByLabelText('Tool status').textContent).toBe(status.type === 'failed' ? 'failure' : status.type === 'outcome_unknown' ? 'uncertain' : 'cancelled');
 }
 ui.rerender(<Tool tool={{ ...tool, tool_id: 'mcp.create_goal' }}/>);
 expect(ui.container.querySelector('[data-goal-activity]')).toBeNull();
 expect(ui.container.querySelector('[data-tool-renderer="generic"]')).toBeTruthy();
});

it('Bash description is safe collapsed text while expanded detail retains command', () => {
 const tool: ForegroundToolExecution = {committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:'a'},block_index:0},assembly_complete:true, message_id: 'a', block_index: 0, call_id: 'description-call', tool_id: 'tool-bash', name: 'bash', state: { type: 'running', arguments: JSON.stringify({ command: 'printf authoritative', description: '<img src=x onerror=alert(1)>' }) } };
 const ui = render(<Tool tool={tool}/>);
 expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeTruthy();
 expect(ui.container.querySelector('img')).toBeNull();
 fireEvent.click(screen.getByText('bash'));
 expect(ui.container.textContent).toContain('printf authoritative');
 const view = toolCard(tool);
 expect(view.state).toBe('running');
 expect(view.input).toBe('printf authoritative');
});
it('image reads have a dedicated renderer and retain managed image references after replay', () => {
 const tool: ForegroundToolExecution = {committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:'a'},block_index:0},assembly_complete:true, message_id: 'a', block_index: 0, call_id: 'image-call', tool_id: 'tool-read-image', name: 'read_image', state: { type: 'settled', arguments: JSON.stringify({ path: 'sample.png' }), result: { status: { type: 'success' }, duration_ms: "1", content: [{ type: 'image', artifact_id: 'artifact_1' }] } } };
 expect(toolCard(tool).variant).toBe('image');
 expect(toolCard(JSON.parse(JSON.stringify(tool)))).toEqual(toolCard(tool));
 const ui = render(<Tool tool={tool}/>);
 expect(ui.container.querySelector('[data-tool-renderer="image"]')).toBeTruthy();
 fireEvent.click(screen.getByText('read_image'));
 expect(ui.container.textContent).toContain('artifact_1');
});


it('preloads the model catalog and reopens the menu without another read or loading state', async () => {
  modelFixture(); server.held.add('session/models'); await server.attached('A'); render(<Control/>);
  const preload = await server.waitFor('session/models', 1);
  await act(async () => server.reply(preload));
  expect(count('session/models')).toBe(1);
  expect(count('session/model')).toBe(0); expect(count('session/checkpoint')).toBe(1);
  await openModels();
  fireEvent.click(screen.getByRole('button', { name: 'Model and thinking effort' }));
  server.held.add('session/models');
  for (let i = 0; i < 3; i++) {
    fireEvent.click(screen.getByRole('button', { name: 'Model and thinking effort' }));
    expect(screen.queryByText('Reading native models…')).toBeNull();
    expect(screen.getByRole('menuitem', { name: 'Model' }).getAttribute('aria-disabled')).not.toBe('true');
    fireEvent.click(screen.getByRole('button', { name: 'Model and thinking effort' }));
  }
  expect(count('session/models')).toBe(1);
});

function SlashControl({ binding = 'default', sessionId = 'A', draftOwner }: { binding?: string; sessionId?: string; draftOwner?: ComposerDraft }) {
 const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
 return <AgentControls client={server.client} view={state.views[sessionId]}>{(toolbar, picker) => <AgentComposer binding={binding} draftOwner={draftOwner} model={toolbar} modelPicker={picker}
   disabled={false} busy={false} active={false} commandAvailable={id => id === 'model'} onCommand={() => {}}
   onSend={async () => false} onUpload={async () => []} onCancel={() => {}}/>}</AgentControls>;
}
async function slashFixture() {
 modelFixture(); await server.attached('A'); server.held.add('session/models'); render(<SlashControl/>);
 const catalog = await server.waitFor('session/models', 1);
 await act(async () => { server.reply(catalog); }); server.held.delete('session/models');
 const input = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
 fireEvent.change(input, { target: { value: '/model' } }); return input;
}
it('slash model/profile choices retain native order across locales, dismiss locally and submit equivalent deliberate selections', async () => {
 const input = await slashFixture(); const baseline = server.requests.length;
 // The Session follows its Model's default: the default-Profile row is its own.
 for (const locale of ['zh', 'en'] as const) {
   act(() => localeController.setLocale(locale));
   expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(['exact/model', 'exact/model / deliberate', 'exact/model / brief', 'other']);
 }
 fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Escape' });
 expect(input.value).toBe('/model'); expect(document.activeElement).toBe(input);
 expect(server.requests).toHaveLength(baseline);
 fireEvent.change(input, { target: { value: '/' } }); fireEvent.change(input, { target: { value: '/model' } });
 await act(async () => fireEvent.click(screen.getByRole('option', { name: 'exact/model / deliberate' })));
 expect(count('session/setModel')).toBe(1); expect(input.value).toBe(''); expect(document.activeElement).toBe(input);
 expect(server.client.getSnapshot().views.A.snapshot?.model?.effective?.profile).toBe('deliberate');
});
it('slash keyboard profile selection on the current model waits for acknowledgement and authoritative reread', async () => {
 const input = await slashFixture(); server.held.add('session/setModel'); server.held.add('session/checkpoint');
 fireEvent.change(screen.getByRole('combobox'), { target: { value: 'bRiEf' } });
 expect(screen.getByRole('option', { name: 'exact/model / brief' })).toBeTruthy();
 await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
 const mutation = await server.waitFor('session/setModel', 1);
 expect(mutation.params).toEqual({ target: server.target('A'), config: { model: 'exact/model', profile: 'brief' } });
 await expect(server.client.send('A', 'too early')).rejects.toThrow('Reread');
 expect(input.value).toBe('/model');
 await act(async () => server.reply(mutation)); const reread = await server.waitFor('session/checkpoint', 2);
 expect(server.client.getSnapshot().views.A.snapshot?.model?.effective?.profile).toBe('deliberate');
 await expect(server.client.send('A', 'still too early')).rejects.toThrow('Reread');
 await act(async () => server.reply(reread));
 expect(server.client.getSnapshot().views.A.snapshot?.model?.effective?.profile).toBe('brief');
 expect(input.value).toBe(''); expect(document.activeElement).toBe(input);
 await server.client.send('A', 'after confirmation'); expect(count('turn/start')).toBe(1); expect(count('session/setModel')).toBe(1);
});

it.each(['original', 'edited', 'replacement'] as const)('acknowledged slash gesture survives its native binding update but preserves %s editor ownership', async editor => {
 modelFixture();
 const commit = server.handlers.get('session/setModel')!;
 server.handlers.set('session/setModel', request => {
   const result = commit(request);
   server.snapshots.get('A')!.model_status = { available: true, body_deferred: false, binding_revision: '2' };
   return result;
 });
 await server.attached('A'); server.held.add('session/models');
 const ui = render(<SlashControl binding="original"/>);
 await act(async () => server.reply(await server.waitFor('session/models', 1)));
 server.held.delete('session/models'); server.held.add('session/setModel'); server.held.add('session/checkpoint');
 const input = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
 fireEvent.change(input, { target: { value: '/model' } });
 await act(async () => fireEvent.click(screen.getByRole('option', { name: 'other' })));
 const mutation = await server.waitFor('session/setModel', 1);
 await expect(server.client.send('A', 'before receipt')).rejects.toThrow('Reread');
 await act(async () => server.reply(mutation));
 const read = await server.waitFor('session/checkpoint', 2);
 expect(input.value).toBe('/model');
 await expect(server.client.send('A', 'before native confirmation')).rejects.toThrow('Reread');
 if (editor === 'replacement') ui.rerender(<SlashControl binding="replacement"/>);
 if (editor !== 'original') fireEvent.change(input, { target: { value: 'Newer authored text' } });
 await act(async () => server.reply(read));
 await waitFor(() => expect(server.client.getSnapshot().views.A.modelMutation).toBeUndefined());
 expect(server.client.getSnapshot().views.A.snapshot?.model_status?.binding_revision).toBe('2');
 expect(server.client.getSnapshot().views.A.snapshot?.model?.configured.model).toBe('other');
 expect(input.value).toBe(editor === 'original' ? '' : 'Newer authored text');
 expect(count('session/setModel')).toBe(1); expect(count('turn/start')).toBe(0);
});

it('a successful model receipt cannot consume its retained editor after another Session owns the view', async () => {
 modelFixture(); await server.attached('A', 'B'); server.held.add('session/models');
 const original = new ComposerDraft(), replacement = new ComposerDraft();
 const ui = render(<SlashControl binding="A" draftOwner={original}/>);
 await act(async () => server.reply(await server.waitFor('session/models', 1)));
 server.held.delete('session/models'); server.held.add('session/setModel'); server.held.add('session/checkpoint');
 fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: '/model' } });
 await act(async () => fireEvent.click(screen.getByRole('option', { name: 'other' })));
 const mutation = await server.waitFor('session/setModel', 1);
 await act(async () => server.reply(mutation));
 const read = await server.waitFor('session/checkpoint', 3);
 ui.rerender(<SlashControl binding="B" sessionId="B" draftOwner={replacement}/>);
 fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'Session B draft' } });
 await act(async () => server.reply(read));
 expect(original.snapshot()?.text).toBe('/model'); expect(replacement.snapshot()?.text).toBe('Session B draft');
 expect(server.client.getSnapshot().views.A.snapshot?.model?.configured.model).toBe('other');
 expect(count('session/setModel')).toBe(1); expect(count('turn/start')).toBe(0);
});
for (const choice of ['exact/model / brief', 'exact/model']) it.each(['refused', 'unknown'] as const)(`slash selection ${choice} %s preserves the invocation and never replays`, async outcome => {
 const input = await slashFixture(); server.held.add('session/setModel');
 const before = server.client.getSnapshot().views.A.snapshot;
 if (outcome === 'refused') server.handlers.set('session/setModel', () => { throw new RpcFailure({ code: -32602, message: 'Selection refused by native' }); });
 await act(async () => fireEvent.click(screen.getByRole('option', { name: choice })));
 const mutation = await server.waitFor('session/setModel', 1);
 expect(server.client.getSnapshot().views.A.snapshot).toBe(before);
 await act(async () => { if (outcome === 'unknown') { server.commit(mutation); server.socket.close(); } else server.reply(mutation); });
 expect(input.value).toBe('/model');
 expect(screen.getAllByRole('alert').some(row => row.textContent?.includes(outcome === 'unknown' ? 'uncertain' : 'Selection refused'))).toBe(true);
 expect(count('session/setModel')).toBe(1);
 if (outcome === 'unknown') {
   await act(async () => server.connect());
   await act(async () => server.client.repairAgentModel('A'));
   expect(server.client.getSnapshot().views.A.snapshot?.model).toEqual(server.snapshots.get('A')!.model);
   expect(server.client.getSnapshot().uncertain).toEqual([expect.objectContaining({ method: 'session/setModel', sessionId: 'A' })]);
   expect(input.value).toBe('/model');
   expect(count('session/setModel')).toBe(1);
 } else {
   expect(server.client.getSnapshot().views.A.snapshot).toBe(before);
   expect(server.client.getSnapshot().uncertain).toEqual([]);
 }
});
it('a late acknowledgement for the current-model gesture cannot report success after its attachment is retired', async () => {
 const input = await slashFixture(); server.held.add('session/setModel');
 const target = server.client.target('A');
 await act(async () => fireEvent.click(screen.getByRole('option', { name: 'exact/model' })));
 const mutation = await server.waitFor('session/setModel', 1);
 expect(mutation.params).toEqual({ target, config: server.snapshots.get('A')!.model!.configured });
 server.commit(mutation);
 await act(async () => { await server.client.release('A'); await server.client.attach('A'); });
 expect(server.client.target('A')).not.toEqual(target);
 const reads = count('session/checkpoint');
 const replacement = server.client.getSnapshot().views.A;
 await act(async () => server.reply(mutation));
 expect(input.value).toBe('/model');
 expect(server.client.getSnapshot().views.A).toBe(replacement);
 expect(count('session/checkpoint')).toBe(reads);
 expect(count('session/setModel')).toBe(1);
});

/** One Model, `example/chat`, whose Profiles resolve like native: a pinned
 * Profile is itself, an absent one follows the catalog's current default. */
function profileFixture(configured: import('../../protocol/app-server/v57').SessionModelConfig, models = ['example/chat'], profiles = ['balanced', 'fast']) {
  const state = { profiles, defaultProfile: profiles.length ? 'balanced' : undefined as string | undefined, revision: 1 };
  const base = cfg3Effective().effective_model;
  const resolve = (config: typeof configured) => ({ ...base, configured: config, effective: { ...base.effective!, model: config.model, profile: config.profile ?? state.defaultProfile ?? null } });
  const catalog = () => models.map(id => ({ model: id, protocol: 'openai_responses' as const, contextWindow: 128000, maxOutputTokens: 8192, declaredCapabilities: base.effective!.declaredCapabilities, effectiveCapabilities: base.effective!.capabilities, credentialSource: { type: 'literal' as const }, profiles: state.profiles.map(id => ({ id, reasoningEnabled: false })), defaultProfile: state.defaultProfile ?? null }));
  const resources = () => ({ revision: String(state.revision), inspection: { definitions: [], resource_diagnostics: [], agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [] } });
  server.snapshots.set('A', { ...snapshot(), resources: resources(), model: resolve(configured) });
  server.handlers.set('session/models', () => (domainReceipt('model_catalog.models',({ models: catalog() }).models??[])));
  server.handlers.set('session/model', () => (domainReceipt('model',[server.snapshots.get('A')!.model!])));
  server.handlers.set('session/setModel', request => {
    if (request.method !== 'session/setModel') throw new Error('wrong request');
    server.snapshots.set('A', { ...server.snapshots.get('A')!, model: resolve(request.params.config) });
    return {type:'model_set',settings_revision:'1'};
  });
  return {
    state,
    /** Native moves the catalog default; a following Session resolves anew. */
    async moveDefault(profile: string) {
      state.defaultProfile = profile;
      server.snapshots.set('A', { ...server.snapshots.get('A')!, model: resolve(server.snapshots.get('A')!.model!.configured) });
      await server.client.refresh('A');
    },
    /** Native publishes a new resource revision whose catalog declares these
     * Profiles; the Session's configured selection is left as it was. */
    async publish(next: string[], defaultProfile?: string) {
      state.profiles = next; state.defaultProfile = defaultProfile; state.revision++;
      server.snapshots.set('A', { ...server.snapshots.get('A')!, resources: resources() });
      await server.client.refresh('A');
    },
  };
}
const unavailable = (profile: string) => screen.queryAllByRole('status').some(row => row.textContent === `Thinking effort ${profile} is unavailable for example/chat in this Workspace.`);
const sent = () => server.requests.filter(row => row.request.method === 'session/setModel').map(row => (row.request as Extract<typeof row.request, { method: 'session/setModel' }>).params.config);
const configuredProfile = () => server.client.getSnapshot().views.A.snapshot?.model?.configured.profile ?? undefined;
async function profileMenu() {
  await openModels();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Thinking effort' }));
  const current = (name: string) => screen.getByRole('menuitemradio', { name }).getAttribute('aria-checked') === 'true';
  return { current, trigger: () => screen.getByRole('button', { name: 'Model and thinking effort' }).textContent };
}
async function chooseRow(name: string) {
  const before = server.requests.length;
  await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name })));
  // A mutation settles with its authoritative reread before the next gesture.
  if (sent().length && server.requests.length > before) await waitFor(() => expect(server.client.getSnapshot().views.A.modelMutation).toBeUndefined());
  await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
}

it.each(['menu', 'slash'] as const)('%s reselects the current model with its complete configuration, waits for native confirmation and never mutates from renders or reads', async surface => {
  const configured = { model: 'example/chat', profile: 'fast', requestParams: { top_k: 40, nested: [null, { seed: 7 }] }, maxOutputTokens: 512,
    summaryModel: { mode: 'explicit' as const, model: 'example/summary', profile: 'short', request_params: { temperature: 0.1 }, max_output_tokens: 64 } };
  const fixture = profileFixture(configured);
  const initial = server.snapshots.get('A')!;
  const frozen = { primary: { ...initial.model!.effective!, requestParams: { original: true }, maxOutputTokens: 777 }, summary: { mode: 'session' as const } };
  initial.attempt = { ...running().attempt!, model: frozen };
  await server.attached('A');
  const ui = render(surface === 'menu' ? <Control/> : <SlashControl/>);
  const chooseCurrent = async () => {
    if (surface === 'menu') {
      await openModels(); fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
      await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: configured.model })));
    } else {
      await waitFor(() => expect(count('session/models')).toBeGreaterThan(0));
      fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: '/model' } });
      await waitFor(() => expect(screen.getByRole('option', { name: configured.model })).toHaveProperty('disabled', false));
      await act(async () => fireEvent.click(screen.getByRole('option', { name: configured.model })));
    }
  };
  server.held.add('session/setModel'); server.held.add('session/checkpoint');
  const before = server.client.getSnapshot().views.A.snapshot;
  await chooseCurrent();
  const mutation = await server.waitFor('session/setModel', 1);
  expect(mutation.params).toEqual({ target: server.target('A'), config: configured });
  expect(server.client.getSnapshot().views.A.snapshot).toBe(before);
  if (surface === 'slash') expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLInputElement).value).toBe('/model');
  ui.rerender(surface === 'menu' ? <Control/> : <SlashControl/>);
  expect(count('session/setModel')).toBe(1);
  await act(async () => server.reply(mutation));
  const reread = await server.waitFor('session/checkpoint', 2);
  expect(server.client.getSnapshot().views.A.snapshot).toBe(before);
  expect(server.client.getSnapshot().views.A.modelMutation?.status).toBe('acknowledged');
  await expect(server.client.send('A', 'before authoritative confirmation')).rejects.toThrow('Reread');
  await act(async () => server.reply(reread));
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.model).toEqual(frozen);
  if (surface === 'slash') expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLInputElement).value).toBe('');
  server.held.delete('session/setModel'); server.held.delete('session/checkpoint');
  await act(async () => fixture.publish(['balanced', 'fast'], 'balanced'));
  expect(count('session/setModel'), 'snapshot and catalog refresh never repeat a gesture').toBe(1);
  await chooseCurrent();
  await waitFor(() => expect(server.client.getSnapshot().views.A.modelMutation).toBeUndefined());
  expect(sent()).toEqual([configured, configured]);
  expect(server.client.getSnapshot().views.A.snapshot?.attempt?.model).toEqual(frozen);
});

it('a Draft equivalent choice stays local, while a deliberate current-model choice during attachment queues the complete native selection', async () => {
  const catalog = modelFixture();
  const configured = { model: 'exact/model', profile: 'brief', requestParams: { temperature: 0.2 }, maxOutputTokens: 777,
    summaryModel: { mode: 'explicit' as const, model: 'other', request_params: { summary_tag: 'keep' }, max_output_tokens: 333 } };
  const source = cfg3Source();
  source.session_models = { kind: 'available', catalog: { models: catalog }, default_model: configured };
  const chosen: typeof configured[] = [];
  const ui = render(<AgentControls client={server.client} draft={{ source, intent: configured, choose: value => chosen.push(value as typeof configured), disabled: false }}/>);
  await openModels(); fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: configured.model })));
  expect(chosen).toEqual([]); expect(count('session/setModel')).toBe(0);
  ui.unmount();

  await server.connect(); server.held.add('session/attach'); server.held.add('session/setModel');
  const attaching = server.client.attach('A');
  const opening = await server.waitFor('session/attach', 1);
  function ConnectingControl() {
    const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
    return <AgentControls client={server.client} view={state.views.A} coldSource={source}/>;
  }
  render(<ConnectingControl/>);
  await openModels(); fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
  await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: configured.model })));
  expect(count('session/setModel'), 'queued intent never dispatches before attachment').toBe(0);
  expect(server.client.getSnapshot().views.A.modelIntent?.config).toEqual(configured);
  await act(async () => { server.reply(opening); await attaching; });
  const mutation = await server.waitFor('session/setModel', 1);
  expect(mutation.params).toEqual({ target: server.target('A'), config: configured });
  await act(async () => server.reply(mutation));
  expect(count('session/setModel')).toBe(1);
});

it('the Session effort pane has one row per native strength and its named default follows native defaults', async () => {
  const independent = { model: 'example/chat', requestParams: { top_k: 40, nested: [null] }, maxOutputTokens: 512, summaryModel: { mode: 'explicit' as const, model: 'example/chat', profile: 'fast', request_params: { temperature: 0.1 } } };
  profileFixture(independent); await server.attached('A'); render(<Control/>);
  let menu = await profileMenu();
  expect(menu.current('balanced')).toBe(true);
  expect(screen.getAllByRole('menuitemradio').map(row => row.textContent)).toEqual(['balanced', 'fast']);
  expect(screen.queryByText(/Model default profile/)).toBeNull();
  await chooseRow('fast');
  expect(sent()).toEqual([{ ...independent, profile: 'fast' }]);
  menu = await profileMenu();
  expect(menu.current('fast')).toBe(true);
  await chooseRow('balanced');
  expect(sent().at(-1)).toEqual(independent);
  expect(configuredProfile()).toBeUndefined();
  menu = await profileMenu();
  expect(menu.current('balanced')).toBe(true);
  await chooseRow('balanced');
  expect(sent()).toEqual([{ ...independent, profile: 'fast' }, independent, independent]);
});

it('choosing the selected Model keeps a pinned Profile; the default action clears it and the effective Profile follows native', async () => {
  profileFixture({ model: 'example/chat', profile: 'fast', maxOutputTokens: 256 }, ['example/chat', 'example/other']);
  await server.attached('A'); render(<Control/>);
  let menu = await profileMenu();
  expect(menu.current('fast')).toBe(true);
  expect(menu.trigger()).toContain('fast');
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await openModels();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
  await chooseRow('example/chat');
  expect(sent()).toEqual([{ model: 'example/chat', profile: 'fast', maxOutputTokens: 256 }]);
  expect(configuredProfile()).toBe('fast');
  menu = await profileMenu();
  await chooseRow('balanced');
  expect(sent().at(-1)).toEqual({ model: 'example/chat', maxOutputTokens: 256 });
  expect(configuredProfile()).toBeUndefined();
  expect(server.client.getSnapshot().views.A.snapshot?.model?.effective?.profile).toBe('balanced');
  menu = await profileMenu();
  expect(menu.current('balanced')).toBe(true);
  expect(menu.trigger()).toContain('balanced');
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  // A different Model starts from its own native defaults.
  await openModels();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
  await chooseRow('example/other');
  expect(sent().at(-1)).toEqual({ model: 'example/other' });
});

it('switching the primary Model resets primary settings and keeps the explicit Summary policy whole; native rejection is shown, not worked around', async () => {
  const summaryModel = { mode: 'explicit' as const, model: 'example/summary', profile: 'short', request_params: { temperature: 0.1, nested: [null] }, max_output_tokens: 64 };
  profileFixture({ model: 'example/chat', profile: 'fast', requestParams: { top_k: 40 }, maxOutputTokens: 256, summaryModel }, ['example/chat', 'example/other']);
  await server.attached('A'); render(<Control/>);
  const chooseModel = async (name: string) => {
    await openModels();
    fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
    await chooseRow(name);
  };
  const configured = () => server.client.getSnapshot().views.A.snapshot?.model?.configured;
  // A → B: the exact whole-state payload carries the complete Summary policy;
  // the primary Profile pin, request overrides and output limit are reset.
  await chooseModel('example/other');
  expect(sent()).toEqual([{ model: 'example/other', summaryModel }]);
  expect(configured()).toEqual({ model: 'example/other', summaryModel });
  // A Profile on the same Model changes only the pin.
  await profileMenu();
  await chooseRow('fast');
  expect(sent().at(-1)).toEqual({ model: 'example/other', summaryModel, profile: 'fast' });
  // Returning to the Model default clears only the pin; repeats reach native.
  await profileMenu();
  await chooseRow('balanced');
  expect(sent().at(-1)).toEqual({ model: 'example/other', summaryModel });
  await profileMenu();
  await chooseRow('balanced');
  await chooseModel('example/other');
  expect(sent()).toHaveLength(5);
  // Native refuses the switch: the refusal is presented and nothing retries
  // with a reset Summary.
  server.handlers.set('session/setModel', () => { throw new RpcFailure({ code: -32602, message: 'Summary refused by native' }); });
  await openModels(); fireEvent.click(screen.getByRole('menuitem', { name: 'Model' })); await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: 'example/chat' })));
  await waitFor(() => expect(screen.getAllByRole('alert').some(row => row.textContent?.includes('Summary refused by native'))).toBe(true));
  expect(sent()).toEqual([
    { model: 'example/other', summaryModel },
    { model: 'example/other', summaryModel, profile: 'fast' },
    { model: 'example/other', summaryModel },
    { model: 'example/other', summaryModel },
    { model: 'example/other', summaryModel },
    { model: 'example/chat', summaryModel },
  ]);
  expect(configured()).toEqual({ model: 'example/other', summaryModel });
});

it('a catalog default change moves a following Session and leaves a pinned one, as native resolves them', async () => {
  const fixture = profileFixture({ model: 'example/chat' }); await server.attached('A'); render(<Control/>);
  await fixture.moveDefault('fast');
  cleanup(); render(<Control/>);
  let menu = await profileMenu();
  expect(menu.current('fast')).toBe(true);
  expect(menu.trigger()).toContain('fast');
  await chooseRow('balanced');
  expect(sent()).toEqual([{ model: 'example/chat', profile: 'balanced' }]);
  await fixture.moveDefault('balanced');
  await fixture.moveDefault('fast');
  cleanup(); render(<Control/>);
  menu = await profileMenu();
  expect(menu.current('balanced')).toBe(true);
  expect(menu.current('fast')).toBe(false);
  expect(menu.trigger()).toContain('balanced');
  expect(sent()).toHaveLength(1);
});

it('a pinned Profile a Model without Profiles no longer declares is identified and cleared explicitly, keeping every other setting', async () => {
  const independent = { model: 'example/chat', requestParams: { top_k: 40 }, maxOutputTokens: 1024, summaryModel: { mode: 'explicit' as const, model: 'example/summary' } };
  profileFixture({ ...independent, profile: 'fast' }, ['example/chat'], []); await server.attached('A'); render(<Control/>);
  let menu = await profileMenu();
  expect(menu.trigger()).toContain('example/chat'); expect(menu.trigger()).toContain('fast');
  expect(unavailable('fast')).toBe(true);
  // The recovery action is the only Profile row; no default Profile is named.
  expect(within(screen.getAllByRole('menu').at(-1)!).getAllByRole('menuitemradio').map(row => row.textContent)).toEqual(['Use model defaults']);
  expect(menu.current('Use model defaults')).toBe(false);
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
  // Choosing the selected Model keeps the pin.
  await openModels();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
  await chooseRow('example/chat');
  expect(sent()).toEqual([{ ...independent, profile: 'fast' }]); expect(configuredProfile()).toBe('fast');
  menu = await profileMenu();
  await chooseRow('Use model defaults');
  expect(sent().at(-1)).toEqual(independent);
  expect(server.client.getSnapshot().views.A.snapshot?.model?.configured).toEqual(independent);
  expect(Object.hasOwn(sent().at(-1)!, 'profile')).toBe(false);
  // Corrected: nothing left to recover, so no Profile submenu and no repeat.
  await openModels();
  expect(screen.queryByRole('menuitem', { name: 'Thinking effort' })).toBeNull();
  expect(unavailable('fast')).toBe(false);
  expect(menu.trigger()).not.toContain('fast');
  expect(sent()).toHaveLength(2);
});

it('a published catalog that removes the pinned Profile leaves it configured and unavailable; a published Profile recovers it with the Summary policy kept', async () => {
  // Issue #459: native publishes the catalog anyway and reports the pin
  // unavailable, keeping the invocation it last resolved as a display fact.
  const independent = { model: 'example/chat', summaryModel: { mode: 'explicit' as const, model: 'example/summary' } };
  const fixture = profileFixture({ ...independent, profile: 'fast' }, ['example/chat'], ['balanced', 'fast', 'deep']); await server.attached('A'); render(<Control/>);
  server.snapshots.set('A', { ...server.snapshots.get('A')!, model: { ...server.snapshots.get('A')!.model!, unavailable: 'model example/chat declares no profile "fast"' } });
  await act(async () => fixture.publish(['balanced', 'deep'], 'balanced'));
  const menu = await profileMenu();
  expect(unavailable('fast')).toBe(true);
  expect(menu.trigger()).toContain('fast');
  // Nothing is presented as in effect in its place.
  expect(menu.current('balanced')).toBe(false);
  expect(menu.current('deep')).toBe(false);
  await chooseRow('deep');
  expect(sent()).toEqual([{ ...independent, profile: 'deep' }]);
  expect(server.client.getSnapshot().views.A.snapshot?.model?.unavailable).toBeUndefined();
  expect(unavailable('fast')).toBe(false);
});

it('a catalog revision that drops a pinned Profile offers recovery while mounted, and an obsolete catalog read cannot restore it', async () => {
  const fixture = profileFixture({ model: 'example/chat', profile: 'fast', maxOutputTokens: 256 }); await server.attached('A'); render(<Control/>);
  let menu = await profileMenu();
  expect(menu.current('fast')).toBe(true);
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
  const reads = count('session/models');
  server.held.add('session/models');
  // A first revision's catalog read, answered while `fast` was still declared,
  // is overtaken by the revision that removes every Profile.
  await act(async () => fixture.publish(['balanced', 'fast'], 'balanced'));
  const overtaken = await server.waitFor('session/models', reads + 1);
  const obsolete = server.commit(overtaken);
  await act(async () => fixture.publish([]));
  const fresh = await server.waitFor('session/models', reads + 2);
  await act(async () => server.reply(fresh));
  await act(async () => server.socket.deliver(obsolete));
  server.held.delete('session/models');
  menu = await profileMenu();
  expect(unavailable('fast')).toBe(true);
  expect(screen.queryByRole('menuitemradio', { name: 'fast' })).toBeNull();
  expect(screen.queryByRole('menuitemradio', { name: /Model default profile/ })).toBeNull();
  await chooseRow('Use model defaults');
  const recovery = server.requests.filter(row => row.request.method === 'session/setModel').map(row => row.request);
  expect(recovery).toHaveLength(1);
  expect(recovery[0].params).toEqual({ target: server.target('A'), config: { model: 'example/chat', maxOutputTokens: 256 } });
  expect(configuredProfile()).toBeUndefined();
  expect(count('session/models')).toBe(reads + 3);
});

it('the menu retry forces a catalog reread with cached choices and a retained rejected cold selection', async () => {
 modelFixture(); await server.connect(); server.held.add('session/attach');
 const attaching = server.client.attach('A');
 const opening = await server.waitFor('session/attach', 1);
 server.handlers.set('session/setModel', () => { throw new RpcFailure({ code: -32000, message: 'Model removed' }); });
 const chosen = server.client.prepareAgentModel('A', { model: 'removed' });
 const rejected = expect(chosen).rejects.toThrow('Model removed');
 server.reply(opening); await attaching; await rejected;
 render(<Control/>); await openModels();
 expect(server.client.getSnapshot().views.A.modelIntent?.phase).toBe('failed');
 const cachedReads = count('session/models');
 expect(cachedReads).toBe(1);
 // Ordinary menu reopening still uses the already authoritative catalog.
 await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Model and thinking effort' })));
 await openModels(); expect(count('session/models')).toBe(cachedReads);
 await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Reread models' })));
 expect(count('session/models')).toBe(cachedReads + 1);
 expect(server.client.getSnapshot().views.A.modelIntent?.phase).toBe('failed');
 expect(count('session/setModel')).toBe(1);
});

it('T13/T19 an unloaded Audit receipt creates no placeholder Chat row or execution outcome', () => {
 const s=running();
 s.attempt!.in_flight={message_id:'deferred-publication',blocks:[{type:'text',block_index:0,text:'old loaded prefix'}]};
 const read_cut={read:{conversation_id:s.conversation_id,journal:'0',transcript:'1',mutation_revision:'0',ledger_through:'0',surface_revision:'0',transcript_total:'1',pending_through:'0',pending_total:'0'},ledger_through:'0',transcript_total:'1',surface_revision:'0',inherited_through:'0'};
 s.publication_settlements=[{settlement:{stream_id:'stream',message_id:'deferred-publication',attempt_id:s.attempt!.attempt_id,turn_id:'turn',request_id:'request',kind:'incomplete',settled_at:'2026-10-13T00:00:00Z',block_count:'9007199254740993'},transcript_cursor:'1',read_cut}];
 const ui=render(<AgentTranscript snapshot={s}/>);
 expect(ui.container.querySelector('[data-publication-settlement]')).toBeNull();
 expect(ui.container.textContent).not.toMatch(/Incomplete assistant response|9007199254740993/i);
 expect(s.publication_settlements[0].settlement.kind).toBe('incomplete');
 expect(ui.container.textContent).not.toContain('old loaded prefix');
 expect(s.attempt!.phase.type).toBe('running');
 expect(s.messages).toEqual([]);
});

it('automatically reads a deferred model configuration before selection and keeps its independent Summary policy', async () => {
 const summaryModel = { mode: 'explicit' as const, model: 'example/summary', request_params: { temperature: 0.1 }, max_output_tokens: 64 };
 profileFixture({ model: 'example/chat', profile: 'fast', summaryModel }, ['example/chat', 'example/other']);
 await server.attached('A');
 const observed = server.client.getSnapshot().views.A.snapshot!;
 observed.model = undefined; observed.model_status = { available: true, body_deferred: true, binding_revision: '1' };
 render(<Control/>); await openModels();
 expect(count('session/model')).toBeGreaterThan(0);
 fireEvent.click(screen.getByRole('menuitem', { name: 'Model' })); await chooseRow('example/other');
 expect(sent()).toEqual([{ model: 'example/other', summaryModel }]);
 expect(screen.queryByText('Loading…', { exact: true })).toBeNull();
});

it('a new native catalog revision retains its current menu and rows until the fenced reread settles', async () => {
 const fixture = profileFixture({ model: 'example/chat', profile: 'fast' });
 await server.attached('A'); render(<Control/>); await profileMenu();
 const card = screen.getByRole('menu'), row = screen.getByRole('menuitemradio', { name: 'fast' });
 const trigger = screen.getByRole('button', { name: 'Model and thinking effort' });
 const before = count('session/models'); server.held.add('session/models');
 await act(async () => fixture.publish(['balanced', 'fast', 'deep'], 'balanced'));
 const read = await server.waitFor('session/models', before + 1);
 expect(screen.getByRole('menu')).toBe(card); expect(screen.getByRole('menuitemradio', { name: 'fast' })).toBe(row);
 expect(row).toHaveProperty('disabled', true); expect(trigger.textContent).toBe('example/chatfast');
 expect(screen.queryByText('Reading native models…')).toBeNull(); expect(screen.queryByRole('menuitemradio', { name: 'deep' })).toBeNull();
 await act(async () => server.reply(read));
 expect(screen.getByRole('menu')).toBe(card); expect(screen.getByRole('menuitemradio', { name: 'fast' })).toBe(row);
 expect(row).toHaveProperty('disabled', false); expect(screen.getByRole('menuitemradio', { name: 'deep' })).toBeTruthy();
 expect(sent()).toEqual([]);
});

it('a deferred configuration reread never replaces the selected model with the Workspace default', async () => {
 modelFixture(); await server.attached('A');
 server.client.getSnapshot().views.A.settings = { cwd: '/workspace/A', model: { model: 'workspace-default' } };
 render(<Control/>); await openModels();
 const trigger = screen.getByRole('button', { name: 'Model and thinking effort' });
 const full = structuredClone(server.snapshots.get('A')!.model!);
 full.configured = { model: 'other' }; full.effective!.model = 'other'; full.effective!.profile = null;
 const apply = server.handlers.get('session/setModel')!;
 server.handlers.set('session/setModel', request => {
   const result = apply(request);
   const next = server.snapshots.get('A')!;
   next.model = undefined; next.model_status = { available: true, body_deferred: true, binding_revision: '2' };
   return result;
 });
 server.handlers.set('session/model', () => domainReceipt('model', [full])); server.held.add('session/model');
 fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
 await act(async () => fireEvent.click(screen.getByRole('menuitemradio', { name: 'other' })));
 await server.waitFor('session/model', 2);
 expect(screen.getByRole('menu')).toBeTruthy(); expect(trigger.getAttribute('aria-busy')).toBe('true');
 expect(trigger.textContent).toContain('exact/model');
 expect(trigger.textContent).not.toContain('workspace-default');
 await act(async () => { server.held.delete('session/model'); for (const row of server.requests.filter(row => row.request.method === 'session/model')) server.reply(row.request); });
 await waitFor(() => expect(trigger.textContent).toContain('other'));
 await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
 expect(sent()).toEqual([{ model: 'other' }]);
});
