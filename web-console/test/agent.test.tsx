import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import type { CatalogModelView, ForegroundToolExecution, RuntimeClientSnapshot } from '../../protocol/app-server/v39';
import { AgentComposer } from '../src/app/agent/AgentComposer';
import { localeController } from '../src/locale/controller';
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
afterEach(() => { cleanup(); server.client.disconnect(); act(() => localeController.setLocale('en')); });
const count = (method: string) => server.requests.filter(row => row.request.method === method).length;
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
 server.handlers.set('session/models', () => ({ type: 'models', catalog: { models: catalog } }));
 server.handlers.set('session/model', () => ({ type: 'model', model: server.snapshots.get('A')!.model! }));
 server.handlers.set('session/setModel', request => {
   if (request.method !== 'session/setModel') throw new Error('wrong request');
   const next = structuredClone(server.snapshots.get('A')!);
   next.model!.configured = request.params.config;
   next.model!.effective!.model = request.params.config.model;
   next.model!.effective!.profile = request.params.config.profile;
   server.snapshots.set('A', next);
   return { type: 'model', model: next.model! };
 });
 return catalog;
}
async function openModels() {
 await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Model and profile' })));
 await waitFor(() => expect(screen.queryByText('Reading native models…')).toBeNull());
}
it('model/profile menu advertises only exact native values and acknowledgement alone never changes selection', async () => {
 modelFixture(); await server.attached('A'); render(<Control/>); await openModels();
 expect(count('session/models')).toBe(1); expect(count('session/model')).toBe(0); expect(count('session/snapshot')).toBe(0);
 fireEvent.click(screen.getByRole('menuitem', { name: 'Profile' }));
 expect(screen.getByRole('menuitem', { name: 'brief' })).toBeTruthy();
 expect(screen.queryByText('high')).toBeNull(); expect(screen.queryByText('off')).toBeNull();
 server.held.add('session/setModel'); server.held.add('session/snapshot');
 await act(async () => fireEvent.click(screen.getByRole('menuitem', { name: 'brief' })));
 const request = await server.waitFor('session/setModel', 1);
 expect(request.params).toEqual({ target: server.target('A'), config: { model: 'exact/model', profile: 'brief' } });
 await act(async () => server.reply(request));
 expect(screen.getByRole('button', { name: 'Model and profile' }).textContent).toContain('deliberate');
 expect(count('session/setModel')).toBe(1);
 expect(server.client.getSnapshot().views.A.modelMutation?.status).toBe('acknowledged');
 await expect(server.client.send('A', 'dependent turn')).rejects.toThrow('Reread native model state');
 expect(count('turn/start')).toBe(0);
 const read = await server.waitFor('session/snapshot', 1);
 await act(async () => server.reply(read));
 expect(screen.getByRole('button', { name: 'Model and profile' }).textContent).toContain('brief');
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
 const expected = server.client.cancellationTarget('A')!;
 await expect(server.client.cancelTurn(expected)).rejects.toThrow('Read unavailable');
 expect(server.client.getSnapshot().views.A.cancellation?.status).toBe('acknowledged');
 expect(server.client.getSnapshot().views.A.snapshot?.attempt?.phase.type).toBe('running');
 server.handlers.delete('session/snapshot'); await server.client.refresh('A');
 await server.client.cancelTurn(expected); expect(count('turn/cancel')).toBe(1);
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

it('Bash description is safe collapsed text while expanded detail retains command', () => {
 const tool: ForegroundToolExecution = { message_id: 'a', block_index: 0, call_id: 'description-call', tool_id: 'tool-bash', name: 'bash', state: { type: 'running', arguments: JSON.stringify({ command: 'printf authoritative', description: '<img src=x onerror=alert(1)>' }) } };
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
 const tool: ForegroundToolExecution = { message_id: 'a', block_index: 0, call_id: 'image-call', tool_id: 'tool-read-image', name: 'read_image', state: { type: 'settled', arguments: JSON.stringify({ path: 'sample.png' }), result: { status: { type: 'success' }, duration_ms: 1, content: [{ type: 'image', artifact_id: 'artifact_1' }] } } };
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
  expect(count('session/model')).toBe(0); expect(count('session/snapshot')).toBe(0);
  await openModels();
  fireEvent.click(screen.getByRole('button', { name: 'Model and profile' }));
  server.held.add('session/models');
  for (let i = 0; i < 3; i++) {
    fireEvent.click(screen.getByRole('button', { name: 'Model and profile' }));
    expect(screen.queryByText('Reading native models…')).toBeNull();
    expect(screen.getByRole('menuitem', { name: 'Model' }).getAttribute('aria-disabled')).not.toBe('true');
    fireEvent.click(screen.getByRole('button', { name: 'Model and profile' }));
  }
  expect(count('session/models')).toBe(1);
});

function SlashControl() {
 const state = useSyncExternalStore(server.client.subscribe, server.client.getSnapshot);
 return <AgentControls client={server.client} view={state.views.A}>{(toolbar, picker) => <AgentComposer model={toolbar} modelPicker={picker}
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
it('slash model/profile choices retain native order and membership across locales, with no RPC on dismissal or equivalent selection', async () => {
 const input = await slashFixture(); const baseline = server.requests.length;
 // The Session follows its Model's default: the default-Profile row is its own.
 for (const [locale, followDefault] of [['zh', '模型默认预设（deliberate）'], ['en', 'Model default profile (deliberate)']] as const) {
   act(() => localeController.setLocale(locale));
   expect(screen.getAllByRole('option').map(row => row.getAttribute('aria-label'))).toEqual(['exact/model', `exact/model / ${followDefault}`, 'exact/model / deliberate', 'exact/model / brief', 'other']);
 }
 fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Escape' });
 expect(input.value).toBe('/model'); expect(document.activeElement).toBe(input);
 expect(server.requests).toHaveLength(baseline);
 fireEvent.change(input, { target: { value: '/' } }); fireEvent.change(input, { target: { value: '/model' } });
 await act(async () => fireEvent.click(screen.getByRole('option', { name: 'exact/model / Model default profile (deliberate)' })));
 expect(count('session/setModel')).toBe(0); expect(input.value).toBe(''); expect(document.activeElement).toBe(input);
 expect(server.client.getSnapshot().views.A.snapshot?.model?.effective?.profile).toBe('deliberate');
});
it('slash keyboard profile selection on the current model waits for acknowledgement and authoritative reread', async () => {
 const input = await slashFixture(); server.held.add('session/setModel'); server.held.add('session/snapshot');
 fireEvent.change(screen.getByRole('combobox'), { target: { value: 'bRiEf' } });
 expect(screen.getByRole('option', { name: 'exact/model / brief' })).toBeTruthy();
 await act(async () => fireEvent.keyDown(screen.getByRole('combobox'), { key: 'Enter' }));
 const mutation = await server.waitFor('session/setModel', 1);
 expect(mutation.params).toEqual({ target: server.target('A'), config: { model: 'exact/model', profile: 'brief' } });
 await expect(server.client.send('A', 'too early')).rejects.toThrow('Reread');
 expect(input.value).toBe('/model');
 await act(async () => server.reply(mutation)); const reread = await server.waitFor('session/snapshot', 1);
 expect(server.client.getSnapshot().views.A.snapshot?.model?.effective?.profile).toBe('deliberate');
 await expect(server.client.send('A', 'still too early')).rejects.toThrow('Reread');
 await act(async () => server.reply(reread));
 expect(server.client.getSnapshot().views.A.snapshot?.model?.effective?.profile).toBe('brief');
 expect(input.value).toBe(''); expect(document.activeElement).toBe(input);
 await server.client.send('A', 'after confirmation'); expect(count('turn/start')).toBe(1); expect(count('session/setModel')).toBe(1);
});
it.each(['refused', 'unknown'] as const)('slash profile mutation %s preserves the invocation and never replays', async outcome => {
 const input = await slashFixture(); server.held.add('session/setModel');
 if (outcome === 'refused') server.handlers.set('session/setModel', () => { throw new RpcFailure({ code: -32602, message: 'Profile refused by native' }); });
 await act(async () => fireEvent.click(screen.getByRole('option', { name: 'exact/model / brief' })));
 const mutation = await server.waitFor('session/setModel', 1);
 await act(async () => { if (outcome === 'unknown') { server.commit(mutation); server.socket.close(); } else server.reply(mutation); });
 expect(input.value).toBe('/model');
 expect(screen.getAllByRole('alert').some(row => row.textContent?.includes(outcome === 'unknown' ? 'uncertain' : 'Profile refused'))).toBe(true);
 expect(count('session/setModel')).toBe(1);
 if (outcome === 'unknown') { await act(async () => server.connect()); expect(count('session/setModel')).toBe(1); }
});

/** One Model, `example/chat`, whose Profiles resolve like native: a pinned
 * Profile is itself, an absent one follows the catalog's current default. */
function profileFixture(configured: import('../../protocol/app-server/v39').SessionModelConfig, models = ['example/chat'], profiles = ['balanced', 'fast']) {
  const state = { profiles, defaultProfile: profiles.length ? 'balanced' : undefined as string | undefined, revision: 1 };
  const base = cfg3Effective().effective_model;
  const resolve = (config: typeof configured) => ({ ...base, configured: config, effective: { ...base.effective!, model: config.model, profile: config.profile ?? state.defaultProfile ?? null } });
  const catalog = () => models.map(id => ({ model: id, protocol: 'openai_responses' as const, contextWindow: 128000, maxOutputTokens: 8192, declaredCapabilities: base.effective!.declaredCapabilities, effectiveCapabilities: base.effective!.capabilities, credentialSource: { type: 'literal' as const }, profiles: state.profiles.map(id => ({ id, reasoningEnabled: false })), defaultProfile: state.defaultProfile ?? null }));
  const resources = () => ({ revision: String(state.revision), inspection: { definitions: [], resource_diagnostics: [], agents: {}, workflows: {}, sources: {}, skills: [], skill_diagnostics: [] } });
  server.snapshots.set('A', { ...snapshot(), resources: resources(), model: resolve(configured) });
  server.handlers.set('session/models', () => ({ type: 'models', catalog: { models: catalog() } }));
  server.handlers.set('session/model', () => ({ type: 'model', model: server.snapshots.get('A')!.model! }));
  server.handlers.set('session/setModel', request => {
    if (request.method !== 'session/setModel') throw new Error('wrong request');
    server.snapshots.set('A', { ...server.snapshots.get('A')!, model: resolve(request.params.config) });
    return { type: 'model', model: server.snapshots.get('A')!.model! };
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
const unavailable = (profile: string) => screen.queryAllByRole('status').some(row => row.textContent === `Profile ${profile} is unavailable for example/chat in this Workspace.`);
const sent = () => server.requests.filter(row => row.request.method === 'session/setModel').map(row => (row.request as Extract<typeof row.request, { method: 'session/setModel' }>).params.config);
const configuredProfile = () => server.client.getSnapshot().views.A.snapshot?.model?.configured.profile ?? undefined;
async function profileMenu() {
  await openModels();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Profile' }));
  const current = (name: string) => screen.getByRole('menuitem', { name }).getAttribute('aria-current') === 'true';
  return { current, trigger: () => screen.getByRole('button', { name: 'Model and profile' }).textContent };
}
async function chooseRow(name: string) {
  const before = server.requests.length;
  await act(async () => fireEvent.click(screen.getByRole('menuitem', { name })));
  // A mutation settles with its authoritative reread before the next gesture.
  if (sent().length && server.requests.length > before) await waitFor(() => expect(server.client.getSnapshot().views.A.modelMutation).toBeUndefined());
  await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
}

it('the Session menu distinguishes following the Model default from pinning a Profile, through a single-Model catalog', async () => {
  const independent = { model: 'example/chat', requestParams: { top_k: 40, nested: [null] }, maxOutputTokens: 512, summaryModel: { mode: 'explicit' as const, model: 'example/chat', profile: 'fast', request_params: { temperature: 0.1 } } };
  profileFixture(independent); await server.attached('A'); render(<Control/>);
  let menu = await profileMenu();
  // Following the default: the default row is the configured choice; the
  // trigger shows what the invocation resolves to.
  expect(menu.current('Model default profile (balanced)')).toBe(true);
  expect(menu.current('balanced')).toBe(false);
  expect(menu.trigger()).toContain('balanced');
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
  // default → fast → default → balanced (explicit) → default, each with the
  // exact whole configuration and no redundant mutation for repeats.
  const steps: [string, typeof independent & { profile?: string }][] = [
    ['fast', { ...independent, profile: 'fast' }],
    ['Model default profile (balanced)', independent],
    ['balanced', { ...independent, profile: 'balanced' }],
    ['balanced', { ...independent, profile: 'balanced' }],
    ['Model default profile (balanced)', independent],
    ['Model default profile (balanced)', independent],
  ];
  const expected: unknown[] = [];
  for (const [row, config] of steps) {
    if (JSON.stringify(expected.at(-1) ?? independent) !== JSON.stringify(config)) expected.push(config);
    menu = await profileMenu();
    await chooseRow(row);
    expect(sent()).toEqual(expected);
    expect(server.client.getSnapshot().views.A.snapshot?.model?.configured).toEqual(config);
  }
  expect(sent()).toHaveLength(4);
  // Pinning the current default: same effective Profile, different intent.
  menu = await profileMenu();
  await chooseRow('balanced');
  expect(configuredProfile()).toBe('balanced');
  expect(server.client.getSnapshot().views.A.snapshot?.model?.effective?.profile).toBe('balanced');
  menu = await profileMenu();
  expect(menu.current('balanced')).toBe(true);
  expect(menu.current('Model default profile (balanced)')).toBe(false);
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
});

it('choosing the selected Model keeps a pinned Profile; the default action clears it and the effective Profile follows native', async () => {
  profileFixture({ model: 'example/chat', profile: 'fast', maxOutputTokens: 256 }, ['example/chat', 'example/other']);
  await server.attached('A'); render(<Control/>);
  let menu = await profileMenu();
  expect(menu.current('fast')).toBe(true);
  expect(menu.trigger()).toContain('fast');
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await openModels();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
  await chooseRow('example/chat');
  expect(sent()).toEqual([]);
  expect(configuredProfile()).toBe('fast');
  menu = await profileMenu();
  await chooseRow('Model default profile (balanced)');
  expect(sent()).toEqual([{ model: 'example/chat', maxOutputTokens: 256 }]);
  expect(configuredProfile()).toBeUndefined();
  expect(server.client.getSnapshot().views.A.snapshot?.model?.effective?.profile).toBe('balanced');
  menu = await profileMenu();
  expect(menu.current('Model default profile (balanced)')).toBe(true);
  expect(menu.trigger()).toContain('balanced');
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
  // Returning to the Model default clears only the pin; repeats are no-ops.
  await profileMenu();
  await chooseRow('Model default profile (balanced)');
  expect(sent().at(-1)).toEqual({ model: 'example/other', summaryModel });
  await profileMenu();
  await chooseRow('Model default profile (balanced)');
  await chooseModel('example/other');
  expect(sent()).toHaveLength(3);
  // Native refuses the switch: the refusal is presented and nothing retries
  // with a reset Summary.
  server.handlers.set('session/setModel', () => { throw new RpcFailure({ code: -32602, message: 'Summary refused by native' }); });
  await act(async () => { await openModels(); fireEvent.click(screen.getByRole('menuitem', { name: 'Model' })); fireEvent.click(screen.getByRole('menuitem', { name: 'example/chat' })); });
  await waitFor(() => expect(screen.getAllByRole('alert').some(row => row.textContent?.includes('Summary refused by native'))).toBe(true));
  expect(sent()).toEqual([
    { model: 'example/other', summaryModel },
    { model: 'example/other', summaryModel, profile: 'fast' },
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
  expect(menu.current('Model default profile (fast)')).toBe(true);
  expect(menu.trigger()).toContain('fast');
  await chooseRow('balanced');
  expect(sent()).toEqual([{ model: 'example/chat', profile: 'balanced' }]);
  await fixture.moveDefault('balanced');
  await fixture.moveDefault('fast');
  cleanup(); render(<Control/>);
  menu = await profileMenu();
  expect(menu.current('balanced')).toBe(true);
  expect(menu.current('Model default profile (fast)')).toBe(false);
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
  expect(within(screen.getAllByRole('menu').at(-1)!).getAllByRole('menuitem').map(row => row.textContent)).toEqual(['Use model defaults']);
  expect(menu.current('Use model defaults')).toBe(false);
  await act(async () => fireEvent.keyDown(document.activeElement!, { key: 'Escape' }));
  await waitFor(() => expect(screen.queryByRole('menu')).toBeNull());
  // Choosing the selected Model keeps the pin.
  await openModels();
  fireEvent.click(screen.getByRole('menuitem', { name: 'Model' }));
  await chooseRow('example/chat');
  expect(sent()).toEqual([]); expect(configuredProfile()).toBe('fast');
  menu = await profileMenu();
  await chooseRow('Use model defaults');
  expect(sent()).toEqual([independent]);
  expect(server.client.getSnapshot().views.A.snapshot?.model?.configured).toEqual(independent);
  expect(Object.hasOwn(sent()[0], 'profile')).toBe(false);
  // Corrected: nothing left to recover, so no Profile submenu and no repeat.
  await openModels();
  expect(screen.queryByRole('menuitem', { name: 'Profile' })).toBeNull();
  expect(unavailable('fast')).toBe(false);
  expect(menu.trigger()).not.toContain('fast');
  expect(sent()).toHaveLength(1);
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
  expect(menu.current('Model default profile (balanced)')).toBe(false);
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
  expect(screen.queryByRole('menuitem', { name: 'fast' })).toBeNull();
  expect(screen.queryByRole('menuitem', { name: /Model default profile/ })).toBeNull();
  await chooseRow('Use model defaults');
  const recovery = server.requests.filter(row => row.request.method === 'session/setModel').map(row => row.request);
  expect(recovery).toHaveLength(1);
  expect(recovery[0].params).toEqual({ target: server.target('A'), config: { model: 'example/chat', maxOutputTokens: 256 } });
  expect(configuredProfile()).toBeUndefined();
  expect(count('session/models')).toBe(reads + 3);
});
