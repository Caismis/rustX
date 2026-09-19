// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { commands, discoveryQuery, parseCommand, available } from '../src/app/commands/registry';
import { activeAttempt, executionIdle, lineageSwitchSafe } from '../src/bindings/projection';
import { matchCommands } from '../src/app/commands/matching';
import { AgentComposer } from '../src/app/agent/AgentComposer';
import { CommandSession, NavigationEpoch, createSession } from '../src/app/commands/native';
import { CommandPanel } from '../src/app/commands/CommandPanel';
import { App } from '../src/app/App';
import { OutcomeUncertain, RpcFailure } from '../src/client/app-server';
import type { MethodResult, Request, SessionNode, SessionUserMessageBoundary, UserInputBlock } from '../../protocol/app-server/v9';
import { Server, snapshot } from './fixture';

let server: Server;
beforeEach(() => {
  server = new Server();
  localStorage.clear();
});
afterEach(() => { cleanup(); server.client.disconnect(); vi.restoreAllMocks(); });
const methods = () => server.requests.map(item => item.request.method);

describe('one narrow browser command grammar', () => {
  it('discovers slash, refuses unsupported input and arguments, and leaves ordinary text alone', () => {
    expect(discoveryQuery('/')).toBe(''); expect(discoveryQuery(' /mdl')).toBe('mdl');
    expect(discoveryQuery('Read /model')).toBeUndefined();
    expect(parseCommand('/not-a-command')).toEqual({ type: 'unsupported' });
    expect(parseCommand('/model anything')).toEqual({ type: 'unsupported' });
    for (const text of ['ordinary text', 'https://example.com/a', 'Read /tmp/file']) expect(parseCommand(text)).toEqual({ type: 'text' });
  });
  it('keeps stable exact identity across display labels and aliases; fuzzy ranking is deterministic', () => {
    expect(parseCommand('/model')).toEqual({ type: 'command', id: 'model' });
    expect(parseCommand('/模型')).toEqual(parseCommand('/model'));
    expect(parseCommand('/approval')).toEqual({ type: 'unsupported' });
    expect(matchCommands('')).toBe(commands);
    expect(matchCommands('mdl').map(item => item.id)).toEqual(['model']);
    expect(matchCommands('权限')).toEqual([]);
    const tied = [{ id: 'fork', label: 'same', aliases: [], availability: 'attached' }, { id: 'branch', label: 'same', aliases: [], availability: 'attached' }] as const;
    expect(matchCommands('same', tied)).toEqual(tied);
    expect(matchCommands('permission')).toEqual([]);
    expect(matchCommands('impossible-command')).toEqual([]);
  });
  it('supports keyboard highlight, Escape, outside dismissal and plus without losing drafts; unknown slash never sends', async () => {
    const send = vi.fn(async () => true), command = vi.fn();
    render(<AgentComposer disabled={false} busy={false} active={false} onCommand={command} onSend={send} onUpload={async () => []} onCancel={() => {}} />);
    const input = screen.getByLabelText('Message'); input.focus();
    fireEvent.change(input, { target: { value: '/' } });
    expect(screen.getByRole('listbox', { name: 'Commands' })).toBeTruthy();
    fireEvent.keyDown(input, { key: 'ArrowDown' }); fireEvent.keyDown(input, { key: 'Enter' });
    expect(command).toHaveBeenLastCalledWith('compact'); expect(document.activeElement).toBe(input);
    fireEvent.change(input, { target: { value: '/mdl' } }); fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByRole('listbox')).toBeNull(); expect((input as HTMLTextAreaElement).value).toBe('/mdl');
    fireEvent.click(screen.getByRole('button', { name: 'Commands' })); expect(screen.getByRole('listbox')).toBeTruthy();
    fireEvent.pointerDown(document.body); expect(screen.queryByRole('listbox')).toBeNull();
    fireEvent.change(input, { target: { value: '/not-a-command' } });
    await act(async () => fireEvent.keyDown(input, { key: 'Enter' }));
    expect(send).not.toHaveBeenCalled(); expect(screen.getByRole('alert').textContent).toContain('Unsupported');
    expect((input as HTMLTextAreaElement).value).toBe('/not-a-command');
    fireEvent.change(input, { target: { value: 'Read /model documentation' } });
    await act(async () => fireEvent.keyDown(input, { key: 'Enter' }));
    expect(send).toHaveBeenCalledWith('Read /model documentation', [], 'send');
  });
});

/** Scripted server owner. The browser only receives generated wire DTOs. */
function nativeFixture() {
  const original = snapshot('A');
  original.transcript.entries = [{ cursor: '1', item: { type: 'message', message: { id: 'original-assistant', role: 'assistant', content: [{ type: 'text', text: 'Original assistant response' }] } } }];
  const originals = structuredClone(original);
  server.snapshots.set('A', original); server.nodeSnapshots.set('node-A', original);
  const content: UserInputBlock[] = [{ type: 'text', text: 'Try this' }, { type: 'upload', session_id: 'A', batch_id: 'batch', token: 'native-receipt' }];
  const boundary: SessionUserMessageBoundary = { surface_revision: '9007199254740997', message: { id: 'user-cut', kind: 'message', source: 'human', content: [{ type: 'text', text: 'Try this' }, { type: 'uploaded_file', batch_id: 'batch', name: 'note.txt' }] } };
  const nodes: SessionNode[] = [{ id: 'node-A', conversation_id: 'conversation-A', ordinal: '1', origin: { type: 'new' } }];
  const committed: Extract<MethodResult, { type: 'session_transition' }>[] = [];
  let model = 'fixture/first';
  const modelView = () => ({ configured: { model }, effective: { model }, summary: { mode: 'session' } }) as Extract<MethodResult, { type: 'model' }>['model'];
  server.handlers.set('settings/model', () => ({ type: 'model', model: modelView() }));
  server.handlers.set('settings/models', () => ({ type: 'models', catalog: { models: [{ model: 'fixture/first' }, { model: 'fixture/second' }] } } as Extract<MethodResult, { type: 'models' }>));
  server.handlers.set('settings/setModel', request => { if (request.method !== 'settings/setModel') throw new Error('wrong method'); model = request.params.config.model; return { type: 'model', model: modelView() }; });
  server.handlers.set('session/boundaries', () => ({ type: 'boundaries', surface_revision: boundary.surface_revision, boundaries: [boundary] }));
  server.handlers.set('session/tree', () => ({ type: 'tree', nodes }));
  const transition = (request: Request): MethodResult => {
    if (request.method !== 'session/fork' && request.method !== 'session/branch') throw new Error('wrong method');
    if (request.params.surface_revision !== boundary.surface_revision || request.params.boundary !== boundary.message.id) throw new RpcFailure({ code: -32602, message: 'Invalid historical Surface revision or user boundary' });
    const id = request.method === 'session/fork' ? 'child' : 'A';
    const nodeId = request.method === 'session/fork' ? 'node-child' : 'branch-A';
    const conversation = request.method === 'session/fork' ? 'conversation-child' : 'conversation-branch-A';
    nodes.push({ ordinal: String(nodes.length + 1), id: nodeId, parent: id === 'A' ? 'node-A' : null, conversation_id: conversation, origin: { type: 'fork', source_session: 'A', source_node: 'node-A', source_surface_revision: boundary.surface_revision, source_user_message: boundary.message.id } });
    if (id === 'child') server.snapshots.set(id, snapshot(id));
    server.nodeSnapshots.set(nodeId, { ...snapshot(), conversation_id: conversation });
    const result: Extract<MethodResult, { type: 'session_transition' }> = { type: 'session_transition', session: { id, active_node: nodeId, active_conversation_id: conversation, node_count: nodes.length, created_at: '0', updated_at: '0' }, editor_content: content.map(block => block.type === 'upload' ? { ...block, session_id: id, token: id === 'child' ? 'native-destination-receipt' : block.token } : block) };
    committed.push(result); return result;
  };
  server.handlers.set('session/fork', transition); server.handlers.set('session/branch', transition);
  server.handlers.set('session/create', () => {
    server.snapshots.set('child', snapshot('child'));
    const result: Extract<MethodResult, { type: 'session_transition' }> = { type: 'session_transition', session: { id: 'child', active_node: 'node-child', active_conversation_id: 'conversation-child', node_count: 1, created_at: '0', updated_at: '0' } };
    committed.push(result); return result;
  });
  server.handlers.set('context/compact', () => ({ type: 'context', context: { compaction_in_progress: false, compaction_count: 1 } }));
  return { original, originals, content, boundary, nodes, committed, model: () => model };
}
async function subject() {
  const fixture = nativeFixture(); await server.attached('A', 'B');
  const navigation = new NavigationEpoch();
  const scope = new CommandSession(server.client, 'A', navigation.capture());
  const selection = (await scope.boundaries()).selections[0];
  return { fixture, navigation, scope, selection };
}
describe('successful command draft consumption', () => {
  async function open(draft: string) {
    await subject();
    localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A'] }));
    render(<App client={server.client} workspaceHost={server.workspaceHost} />);
    const input = screen.getByLabelText('Message'); input.focus();
    fireEvent.change(input, { target: { value: draft } });
    await act(async () => fireEvent.keyDown(input, { key: 'Enter' }));
    return input;
  }
  it.each(['/model', '/mdl', '/', '/模型'])('%s is consumed only after successful selection', async draft => {
    const input = await open(draft);
    expect(input).toHaveProperty('value', draft);
    await act(async () => fireEvent.click(await screen.findByRole('option', { name: /fixture\/second/ })));
    expect(input).toHaveProperty('value', ''); expect(screen.queryByRole('dialog')).toBeNull();
    expect(document.activeElement).toBe(input);
  });
  it('dismissal preserves a fuzzy invocation', async () => {
    const input = await open('/mdl');
    fireEvent.keyDown(document, { key: 'Escape' });
    expect(input).toHaveProperty('value', '/mdl'); expect(document.activeElement).toBe(input);
  });
  it('known model refusal preserves its fuzzy invocation', async () => {
    const input = await open('/mdl');
    server.handlers.set('settings/setModel', () => { throw new RpcFailure({ code: -32602, message: 'Model refused' }); });
    await act(async () => fireEvent.click(await screen.findByRole('option', { name: /fixture\/second/ })));
    expect(screen.getByRole('alert').textContent).toContain('Model refused');
    expect(input).toHaveProperty('value', '/mdl');
  });
  it('a draft edited while selection is pending survives successful completion', async () => {
    const input = await open('/mdl'); server.held.add('settings/setModel');
    fireEvent.click(await screen.findByRole('option', { name: /fixture\/second/ }));
    const request = await server.waitFor('settings/setModel', 1);
    fireEvent.change(input, { target: { value: 'new text' } });
    await act(async () => server.reply(request));
    expect(input).toHaveProperty('value', 'new text'); expect(screen.queryByRole('dialog')).toBeNull();
  });
  it.each([false, true])('/tools read failure=%s consumes only on successful load and keeps its panel open', async failure => {
    server.held.add('resources/read');
    server.handlers.set('resources/read', () => {
      if (failure) throw new RpcFailure({ code: -32000, message: 'Capability read refused' });
      return { type: 'capabilities', capabilities: { revision: '0' } };
    });
    const input = await open('/tools');
    expect(input).toHaveProperty('value', '/tools');
    await act(async () => server.reply(await server.waitFor('resources/read', 1)));
    expect(input).toHaveProperty('value', failure ? '/tools' : '');
    expect(screen.getByRole('dialog')).toBeTruthy();
    if (failure) expect(screen.getByRole('alert').textContent).toContain('Capability read refused');
  });
});

describe('inbound transport frontier', () => {
  it('counts inbound already in the bounded client pipeline before a socket slot is available', async () => {
    await subject(); server.held.add('resources/read'); server.held.add('turn/start');
    server.handlers.set('resources/read', () => ({ type: 'capabilities', capabilities: { revision: '0' } }));
    const reads = Array.from({ length: 8 }, () => server.client.request({ method: 'resources/read', params: { target: server.client.target('A') } }, 'capabilities'));
    const work = server.client.send('A', 'queued transport');
    expect(methods()).not.toContain('turn/start');
    expect(server.client.getSnapshot().views.A.inboundRequests).toBe(1);
    expect(lineageSwitchSafe(server.client.getSnapshot().views.A)).toBe(false);
    for (let index = 1; index <= 8; index++) server.reply(await server.waitFor('resources/read', index));
    server.reply(await server.waitFor('turn/start', 1));
    await Promise.all([...reads, work]);
    expect(server.client.getSnapshot().views.A.inboundRequests).toBe(0);
    expect(lineageSwitchSafe(server.client.getSnapshot().views.A)).toBe(false);
  });
  it.each(['send', 'steer'] as const)('%s commit without response delivery blocks lineage, then atomically hands off to acknowledged identity', async delivery => {
    const { scope, selection, fixture } = await subject();
    const view = () => server.client.getSnapshot().views.A;
    const method = delivery === 'send' ? 'turn/start' : 'turn/steer';
    server.held.add(method);
    const states: boolean[] = [];
    const unsubscribe = server.client.subscribe(() => states.push(lineageSwitchSafe(view())));
    const work = server.client.send('A', 'accepted task', [], delivery);
    expect(view().inboundRequests).toBe(1); expect(executionIdle(view())).toBe(true);
    expect(lineageSwitchSafe(view())).toBe(false);
    expect(lineageSwitchSafe(server.client.getSnapshot().views.B)).toBe(true);
    const request = await server.waitFor(method, 1), response = server.commit(request);
    expect(view().submissions ?? []).toEqual([]);
    await expect(scope.transition('branch', selection)).rejects.toThrow('accepted inbound');
    await expect(scope.transition('retry', selection)).rejects.toThrow('accepted inbound');
    await expect(scope.openNode('other', 'other-conversation')).rejects.toThrow('accepted inbound');
    expect(methods()).not.toContain('session/branch'); expect(methods()).not.toContain('session/switchNode');
    expect(methods().filter(method => method === 'session/attach')).toHaveLength(2);
    server.socket.deliver(response); await work;
    expect(view().inboundRequests).toBe(0);
    expect(view().submissions?.map(item => item.messageId)).toEqual(['accepted-user']);
    const message = { id: 'accepted-user', source: 'human' as const, content: [{ type: 'text' as const, text: 'accepted task' }] };
    await server.update('A', { ...fixture.original, inbound: { pending: [{ revision: '0', sequence: '1', message }] } });
    expect(view().submissions).toEqual([]); expect(lineageSwitchSafe(view())).toBe(false);
    unsubscribe(); expect(states.length).toBeGreaterThan(1); expect(states.every(safe => !safe)).toBe(true);
    await server.update('A', { ...fixture.original, messages: [{ role: 'user', ...message }], inbound: {} });
    expect(lineageSwitchSafe(view())).toBe(true);
  });
  it('concurrent known refusals decrement exact pending ownership without creating submissions', async () => {
    await subject(); server.held.add('turn/start'); server.held.add('turn/steer');
    const fail = () => { throw new RpcFailure({ code: -32602, message: 'Known native refusal' }); };
    server.handlers.set('turn/start', fail); server.handlers.set('turn/steer', fail);
    const first = server.client.send('A', 'one'), second = server.client.send('A', 'two', [], 'steer');
    const rejectedFirst = expect(first).rejects.toBeInstanceOf(RpcFailure), rejectedSecond = expect(second).rejects.toBeInstanceOf(RpcFailure);
    const view = () => server.client.getSnapshot().views.A;
    expect(view().inboundRequests).toBe(2);
    server.reply(await server.waitFor('turn/start', 1)); await rejectedFirst;
    expect(view().inboundRequests).toBe(1); expect(lineageSwitchSafe(view())).toBe(false);
    server.reply(await server.waitFor('turn/steer', 1)); await rejectedSecond;
    expect(view().inboundRequests).toBe(0); expect(view().submissions ?? []).toEqual([]);
    expect(lineageSwitchSafe(view())).toBe(true);
  });
  it('lost inbound response is uncertain, fences old commands, and reconnect clears only old transport ownership', async () => {
    const { scope, selection, fixture } = await subject(); server.held.add('turn/start');
    const work = server.client.send('A', 'possibly accepted'), rejected = expect(work).rejects.toBeInstanceOf(OutcomeUncertain);
    const response = server.commit(await server.waitFor('turn/start', 1)), oldSocket = server.socket;
    const generation = server.client.getSnapshot().generation;
    server.client.disconnect(); await rejected;
    expect(server.client.getSnapshot().generation).toBeGreaterThan(generation);
    expect(lineageSwitchSafe(server.client.getSnapshot().views.A)).toBe(false);
    await expect(scope.transition('branch', selection)).rejects.toThrow('Obsolete');
    const pending = { ...fixture.original, inbound: { pending: [{ revision: "0", sequence: '1', message: { id: 'accepted-user', source: 'human' as const, content: [{ type: 'text' as const, text: 'possibly accepted' }] } }] } };
    server.snapshots.set('A', pending); server.nodeSnapshots.set('node-A', pending);
    await server.connect();
    expect(server.client.getSnapshot().views.A.inboundRequests ?? 0).toBe(0);
    expect(lineageSwitchSafe(server.client.getSnapshot().views.A)).toBe(false);
    const repaired = server.client.getSnapshot(); oldSocket.deliver(response); expect(server.client.getSnapshot()).toBe(repaired);
    expect(methods().filter(method => method === 'turn/start')).toHaveLength(1);
    expect(repaired.uncertain.some(item => item.method === 'turn/start')).toBe(true);
    await server.update('A', fixture.original);
    expect(lineageSwitchSafe(server.client.getSnapshot().views.A)).toBe(true);
  });
  it('Fork and Compact remain allowed while source inbound acknowledgement is held', async () => {
    const { scope, selection } = await subject(); server.held.add('turn/start');
    const work = server.client.send('A', 'in transit');
    const request = await server.waitFor('turn/start', 1);
    await scope.compact();
    expect((await scope.transition('fork', selection))?.session.id).toBe('child');
    expect(methods()).not.toContain('session/switchNode');
    server.reply(request); await work;
  });
  it.each(['branch', 'retry'] as const)('%s committed before an unresolved inbound request stops before unload', async action => {
    const { scope, selection, fixture } = await subject(); server.held.add('session/branch'); server.held.add('turn/start');
    const branch = scope.transition(action, selection), rejected = expect(branch).rejects.toThrow('Branch branch-A committed');
    const branchResponse = server.commit(await server.waitFor('session/branch', 1));
    const send = server.client.send('A', 'racing inbound');
    const turn = await server.waitFor('turn/start', 1), turnResponse = server.commit(turn);
    server.socket.deliver(branchResponse); await rejected;
    expect(fixture.committed).toHaveLength(1); expect((await scope.tree()).nodes.some(node => node.id === 'branch-A')).toBe(true);
    expect(methods()).not.toContain('session/switchNode');
    expect(methods().filter(method => method === 'session/branch')).toHaveLength(1);
    server.socket.deliver(turnResponse); await send;
  });
});

describe('restored native editor content', () => {
  const uploads: UserInputBlock[] = ['one', 'two'].map(token => ({ type: 'upload', session_id: 'A', batch_id: 'same-batch', token }));
  it.each([0, 1])('same-batch receipts have independent stable identities; remove index %s', async index => {
    const errors = vi.spyOn(console, 'error');
    await server.attached('A');
    const send = vi.fn(async (text, receipts, delivery) => { await server.client.send('A', text, receipts, delivery); return true; });
    render(<AgentComposer disabled={false} busy={false} active={false} initialContent={[...uploads, { type: 'text', text: 'unchanged text' }]} onSend={send} onUpload={async () => []} onCancel={() => {}} />);
    expect(screen.getAllByText('Native restored upload batch same-batch')).toHaveLength(2);
    fireEvent.click(screen.getAllByRole('button', { name: 'Remove draft upload' })[index]);
    expect(screen.getAllByText('Native restored upload batch same-batch')).toHaveLength(1);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
    const request = await server.waitFor('turn/start', 1);
    expect(request.params).toMatchObject({ content: [uploads[1 - index], { type: 'text', text: 'unchanged text' }] });
    expect(errors).not.toHaveBeenCalled();
  });
  it('supported restored input round-trips exact native order without changes', async () => {
    await server.attached('A');
    const content: UserInputBlock[] = [...uploads, { type: 'text', text: 'original\ntext' }];
    render(<AgentComposer disabled={false} busy={false} active={false} initialContent={content} onSend={async (text, receipts, delivery) => { await server.client.send('A', text, receipts, delivery); return true; }} onUpload={async () => []} onCancel={() => {}} />);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
    expect((await server.waitFor('turn/start', 1)).params).toMatchObject({ content });
  });
  it.each([
    [{ type: 'text', text: 'before' }, uploads[0], { type: 'text', text: 'after' }],
    [{ type: 'text', text: 'one' }, { type: 'text', text: 'two' }],
    [uploads[0], { type: 'text', text: '' }],
  ] as UserInputBlock[][])('refuses an unrepresentable native shape without reordering: %j', async (...content) => {
    const original = structuredClone(content), send = vi.fn(async () => true);
    render(<AgentComposer disabled={false} busy={false} active={false} initialContent={content} onSend={send} onUpload={async () => []} onCancel={() => {}} />);
    expect(screen.getByRole('alert').textContent).toContain('Cannot restore this ordered native input');
    expect(screen.getByLabelText('Message')).toHaveProperty('disabled', true);
    expect(screen.queryByRole('button', { name: 'Remove draft upload' })).toBeNull();
    await act(async () => fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter' }));
    expect(send).not.toHaveBeenCalled(); expect(content).toEqual(original);
  });
});

describe('typed native operations and continuation fencing', () => {
  it.each(['acknowledgement', 'pending', 'pending-settled'] as const)('%s without an active Attempt blocks lineage until authoritative reconciliation reaches genuine idle', async source => {
    const { scope, selection, fixture } = await subject();
    const history = structuredClone(fixture.original);
    if (source === 'pending-settled') history.attempt = { attempt_id: 'settled-attempt', turn: 1, phase: { type: 'settled', outcome: { type: 'completed', finish_reason: { type: 'stop' } } } };
    history.transcript.entries!.unshift({ cursor: '0', item: { type: 'message', message: { role: 'user', source: 'human', id: 'user-cut', content: [{ type: 'text', text: 'Try this' }] } } });
    await server.update('A', history);
    localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A'] }));
    render(<App client={server.client} workspaceHost={server.workspaceHost} />);
    const pending = { revision: "0", sequence: '1', message: { id: 'accepted-user', source: 'human' as const, content: [{ type: 'text' as const, text: 'Accepted task' }] } };
    if (source === 'acknowledgement') {
      server.held.add('turn/start');
      let work!: Promise<unknown>;
      act(() => { work = server.client.send('A', 'Accepted task'); });
      const request = await server.waitFor('turn/start', 1);
      expect(server.client.getSnapshot().views.A.submissions ?? []).toEqual([]);
      for (const name of ['Branch', 'Retry / Regenerate']) expect(screen.getByRole('button', { name })).toHaveProperty('disabled', true);
      fireEvent.click(screen.getByRole('button', { name: 'Session actions' }));
      expect(screen.getByRole('menuitem', { name: 'Session tree' })).toHaveProperty('disabled', true);
      fireEvent.keyDown(document, { key: 'Escape' });
      expect(screen.getByRole('button', { name: 'Fork' })).toHaveProperty('disabled', false);
      fireEvent.change(screen.getByLabelText('Message'), { target: { value: '/branch' } });
      expect(screen.queryByRole('option', { name: /Branch within/ })).toBeNull();
      await act(async () => { server.reply(request); await work; });
      expect(server.client.getSnapshot().views.A.submissions?.map(item => item.messageId)).toEqual(['accepted-user']);
      expect(server.client.getSnapshot().views.A.snapshot?.inbound.pending ?? []).toEqual([]);
    } else await act(() => server.update('A', { ...history, inbound: { pending: [pending] } }));
    const view = () => server.client.getSnapshot().views.A;
    expect(activeAttempt(view().snapshot)).toBe(false);
    expect(executionIdle(view())).toBe(false);
    if (source !== 'acknowledgement') expect(view().submissions ?? []).toEqual([]);
    for (const name of ['Branch', 'Retry / Regenerate']) expect(screen.getByRole('button', { name })).toHaveProperty('disabled', true);
      fireEvent.click(screen.getByRole('button', { name: 'Session actions' }));
      expect(screen.getByRole('menuitem', { name: 'Session tree' })).toHaveProperty('disabled', true);
      fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.getByRole('button', { name: 'Fork' })).toHaveProperty('disabled', false);
    const input = screen.getByLabelText('Message');
    fireEvent.change(input, { target: { value: '/branch' } });
    expect(screen.queryByRole('option', { name: /Branch within/ })).toBeNull();
    await act(async () => fireEvent.keyDown(input, { key: 'Enter' }));
    expect(screen.getByRole('alert').textContent).toContain('unavailable');
    await expect(scope.transition('branch', selection)).rejects.toThrow('accepted inbound');
    await expect(scope.transition('retry', selection)).rejects.toThrow('accepted inbound');
    await expect(scope.openNode('other-node', 'other-conversation')).rejects.toThrow('accepted inbound');
    expect(methods()).not.toContain('session/branch'); expect(methods()).not.toContain('session/switchNode');
    expect(available(commands.find(command => command.id === 'compact')!, false, false, executionIdle(view()))).toBe(true);
    await act(() => scope.compact()); // native maintenance permits pending inbound
    expect(methods().filter(method => method === 'context/compact')).toHaveLength(1);
    await act(() => server.update('A', { ...history, inbound: { pending: [pending] } }));
    expect(view().submissions ?? []).toEqual([]); // exact MessageId reconciliation, not browser removal
    expect(executionIdle(view())).toBe(false); // authority now owns the blocking fact
    await act(() => server.update('A', { ...history, messages: [{ role: 'user', ...pending.message }], inbound: {} }));
    expect(executionIdle(view())).toBe(true);
    for (const name of ['Branch', 'Retry / Regenerate']) expect(screen.getByRole('button', { name })).toHaveProperty('disabled', false);
    fireEvent.click(screen.getByRole('button', { name: 'Session actions' }));
    expect(screen.getByRole('menuitem', { name: 'Session tree' })).toHaveProperty('disabled', false);
    fireEvent.keyDown(document, { key: 'Escape' });
    fireEvent.change(input, { target: { value: '/' } });
    expect(screen.getByRole('option', { name: /Branch within/ })).toBeTruthy();
  });
  it.each(['branch', 'retry'] as const)('%s committed before concurrent acknowledgement remains discoverable without unloading accepted work', async action => {
    const { scope, selection, fixture } = await subject();
    server.held.add('session/branch');
    const work = scope.transition(action, selection);
    const rejection = expect(work).rejects.toThrow('Branch branch-A committed');
    const request = await server.waitFor('session/branch', 1), response = server.commit(request);
    await server.client.send('A', 'Concurrent accepted task');
    expect(activeAttempt(server.client.getSnapshot().views.A.snapshot)).toBe(false);
    server.socket.deliver(response); await rejection;
    expect(fixture.committed).toHaveLength(1);
    expect((await scope.tree()).nodes.some(node => node.id === 'branch-A')).toBe(true);
    expect(methods().filter(method => method === 'session/branch')).toHaveLength(1);
    expect(methods()).not.toContain('session/switchNode');
    expect(methods().filter(method => method === 'turn/start')).toHaveLength(1);
    expect(server.client.target('A').conversation_id).toBe('conversation-A');
  });
  it('independent Fork remains safe with accepted source work', async () => {
    const { scope, selection } = await subject();
    await server.client.send('A', 'Accepted task');
    expect(executionIdle(server.client.getSnapshot().views.A)).toBe(false);
    expect((await scope.transition('fork', selection))?.session.id).toBe('child');
    expect(methods()).not.toContain('session/switchNode');
    expect(server.client.getSnapshot().views.A.submissions?.[0].messageId).toBe('accepted-user');
  });
  it.each(['branch', 'retry', 'tree'] as const)('an already open %s selector tracks acknowledgement and authoritative reconciliation', async id => {
    const { navigation, fixture } = await subject();
    render(<CommandPanel request={{ id }} client={server.client} sessionId="A" current={navigation.capture()} close={() => {}} succeeded={() => {}} opened={() => {}} />);
    const row = await screen.findByRole('option');
    expect(row).toHaveProperty('disabled', false);
    server.held.add('turn/start');
    let work!: Promise<unknown>;
    act(() => { work = server.client.send('A', 'accepted'); });
    const request = await server.waitFor('turn/start', 1), response = server.commit(request);
    expect(row).toHaveProperty('disabled', true);
    fireEvent.click(row);
    expect(methods()).not.toContain('session/branch'); expect(methods()).not.toContain('session/switchNode');
    await act(async () => { server.socket.deliver(response); await work; });
    expect(row).toHaveProperty('disabled', true);
    await act(() => server.update('A', { ...fixture.original, messages: [{ role: 'user', source: 'human', id: 'accepted-user', content: [{ type: 'text', text: 'accepted' }] }] }));
    expect(row).toHaveProperty('disabled', false);
  });
  it('selects native model identities without any command-string RPC', async () => {
    const { scope, fixture } = await subject();
    expect((await scope.models()).catalog.models?.map(model => model.model)).toEqual(['fixture/first', 'fixture/second']);
    await scope.setModel('fixture/second');
    expect(fixture.model()).toBe('fixture/second');
    expect(methods()).toContain('settings/models'); expect(methods()).toContain('settings/model');
    expect(methods().some(method => method.includes('command'))).toBe(false);
    expect(JSON.stringify(server.requests.map(item => item.request.params))).not.toContain('/model');
  });
  it.each(['session/create', 'context/compact'] as const)('%s uses its native owner and cannot continue across lost responses', async method => {
    const { scope, fixture } = await subject(); server.held.add(method);
    const work = method === 'session/create' ? createSession(server.client, '/workspace/A', scope.current) : scope.compact();
    const rejected = expect(work).rejects.toBeInstanceOf(OutcomeUncertain);
    const request = await server.waitFor(method, 1); server.commit(request); server.client.disconnect(); await rejected;
    server.held.delete(method); await server.connect();
    expect(methods().filter(item => item === method)).toHaveLength(1);
    if (method === 'session/create') expect(fixture.committed).toHaveLength(1);
    expect(server.client.getSnapshot().uncertain.some(item => item.method === method)).toBe(true);
  });
  it('a new Session committed after navigation does not acquire a child attachment', async () => {
    const { scope, navigation, fixture } = await subject(); server.held.add('session/create');
    const work = createSession(server.client, '/workspace/A', scope.current), request = await server.waitFor('session/create', 1);
    const response = server.commit(request); navigation.invalidate(); server.socket.deliver(response);
    expect(await work).toBeUndefined(); expect(fixture.committed).toHaveLength(1);
    expect(methods().filter(method => method === 'session/attach')).toHaveLength(2);
  });
  it.each(['settings/setModel'] as const)('late %s commits but cannot reread or affect the navigated UI', async method => {
    const { scope, navigation, fixture } = await subject(); server.held.add(method);
    const work = scope.setModel('fixture/second');
    const request = await server.waitFor(method, 1); const response = server.commit(request);
    navigation.invalidate(); const count = server.requests.length;
    server.socket.deliver(response); await work;
    expect(server.requests).toHaveLength(count);
    expect(server.client.getSnapshot().views.B.snapshot?.effective_approval_mode).toBe('policy');
    if (method === 'settings/setModel') expect(fixture.model()).toBe('fixture/second');
  });
  it.each(['fork', 'branch', 'retry'] as const)('%s commit/held response/navigation/release leaves the mutation valid without continuation', async action => {
    const { scope, selection, navigation, fixture } = await subject();
    const method = action === 'fork' ? 'session/fork' : 'session/branch'; server.held.add(method);
    const work = scope.transition(action, selection); const request = await server.waitFor(method, 1);
    expect(request.params).toEqual({ session_id: 'A', node_id: 'node-A', surface_revision: fixture.boundary.surface_revision, boundary: 'user-cut' });
    const response = server.commit(request); expect(fixture.committed).toHaveLength(1);
    navigation.invalidate(); const count = server.requests.length;
    server.socket.deliver(response); expect(await work).toBeUndefined();
    expect(server.requests).toHaveLength(count); expect(fixture.original).toEqual(fixture.originals);
    expect(server.client.getSnapshot().views.A.target?.conversation_id).toBe('conversation-A');
  });
  it('rejects the exact invalid revision without refreshing or replacing it', async () => {
    const { scope, selection } = await subject();
    selection.boundary = { ...selection.boundary, surface_revision: '999999999999999999' };
    await expect(scope.transition('fork', selection)).rejects.toThrow('Invalid historical Surface');
    expect(methods().filter(method => method === 'session/fork')).toHaveLength(1);
    expect(methods().filter(method => method === 'session/boundaries')).toHaveLength(1);
  });
  it('Fork attaches only after success and uses destination receipts supplied by the native owner', async () => {
    const { scope, selection, fixture } = await subject(); server.held.add('session/fork');
    const work = scope.transition('fork', selection); const request = await server.waitFor('session/fork', 1);
    expect(methods().filter(method => method === 'session/attach')).toHaveLength(2);
    server.reply(request); const result = await work;
    expect(result?.session.id).toBe('child');
    expect(result?.content[1]).toMatchObject({ type: 'upload', session_id: 'child', token: 'native-destination-receipt' });
    expect(fixture.original).toEqual(fixture.originals); expect(methods()).not.toContain('session/upload');
    expect(methods()).not.toContain('turn/start');
  });
  it.each(['branch', 'retry'] as const)('%s creates a native node, unloads then attaches exactly; only retry executes returned input once', async action => {
    const { scope, selection, fixture } = await subject();
    server.held.add('session/branch'); server.held.add('session/switchNode'); server.held.add('session/attach');
    const work = scope.transition(action, selection);
    const branch = await server.waitFor('session/branch', 1); server.reply(branch);
    const unload = await server.waitFor('session/switchNode', 1);
    expect(methods()).not.toContain('turn/start'); server.reply(unload);
    const attach = await server.waitFor('session/attach', 3);
    expect(attach.params).toEqual({ session_id: 'A', node_id: 'branch-A' });
    expect(methods()).not.toContain('turn/start'); server.reply(attach);
    const result = await work;
    expect(result?.session.active_conversation_id).toBe('conversation-branch-A');
    expect(server.client.target('A').conversation_id).toBe('conversation-branch-A');
    expect(fixture.original).toEqual(fixture.originals);
    expect(server.client.getSnapshot().views.A.history?.page.entries).toEqual([]);
    if (action === 'retry') {
      const turn = await server.waitFor('turn/start', 1);
      expect(turn.params).toEqual({ target: server.client.target('A'), content: fixture.content });
      expect(methods().filter(method => method === 'turn/start')).toHaveLength(1);
      expect(result?.content).toEqual([]);
    } else { expect(methods()).not.toContain('turn/start'); expect(result?.content).toEqual(fixture.content); }
    expect(methods()).not.toContain('session/upload');
  });
  it.each(['session/switchNode', 'session/attach', 'turn/start'] as const)('retry stops after lost %s response and never repeats the branch or execution', async method => {
    const { scope, selection, fixture } = await subject(); server.held.add(method);
    const count = server.requests.filter(item => item.request.method === method).length + 1;
    const work = scope.transition('retry', selection); const rejection = expect(work).rejects.toBeInstanceOf(OutcomeUncertain);
    const request = await server.waitFor(method, count); server.commit(request); server.client.disconnect(); await rejection;
    expect(fixture.committed).toHaveLength(1);
    expect(methods().filter(item => item === 'turn/start')).toHaveLength(method === 'turn/start' ? 1 : 0);
    expect(server.client.getSnapshot().uncertain.some(item => item.method === method)).toBe(true);
  });
  it.each(['session/fork', 'session/branch', 'settings/setModel'] as const)('%s response loss is uncertain, never replayed, and reconnect repairs authority', async method => {
    const { scope, selection, fixture } = await subject(); server.held.add(method);
    const work = method === 'settings/setModel' ? scope.setModel('fixture/second') : scope.transition(method === 'session/fork' ? 'fork' : 'branch', selection);
    const rejection = expect(work).rejects.toBeInstanceOf(OutcomeUncertain);
    const request = await server.waitFor(method, 1); const response = server.commit(request), oldSocket = server.socket;
    server.client.disconnect(); await rejection;
    expect(server.client.getSnapshot().uncertain.some(item => item.method === method)).toBe(true);
    server.held.delete(method); await server.connect();
    const reread = server.client.getSnapshot(); oldSocket.deliver(response);
    expect(server.client.getSnapshot()).toBe(reread);
    expect(methods().filter(item => item === method)).toHaveLength(1); expect(scope.current()).toBe(false);
    const repaired = new CommandSession(server.client, 'A', () => true);
    expect((await repaired.models()).current.configured.model).toBe(fixture.model());
    if (method === 'session/branch') expect((await repaired.tree()).nodes).toHaveLength(2);
    if (method === 'session/fork') expect(server.client.getSnapshot().sessions.some(session => session.id === 'child')).toBe(true);
    await expect(scope.setModel('fixture/first')).rejects.toThrow('Obsolete');
  });
  it('renders and filters native selector options, dispatches Enter, and visibly locks a rejected stale mutation', async () => {
    const { navigation } = await subject(); const close = vi.fn();
    render(<CommandPanel request={{ id: 'model' }} client={server.client} sessionId="A" current={navigation.capture()} close={close} succeeded={() => {}} opened={() => {}} />);
    await screen.findByRole('option', { name: /fixture\/second/ });
    fireEvent.change(screen.getByLabelText('Filter options'), { target: { value: 'second' } });
    expect(screen.getAllByRole('option')).toHaveLength(1);
    await act(async () => fireEvent.keyDown(screen.getByLabelText('Filter options'), { key: 'Enter' }));
    expect(close).toHaveBeenCalledTimes(1);
    cleanup();
    server.handlers.set('session/fork', () => { throw new RpcFailure({ code: -32602, message: 'Invalid historical Surface revision' }); });
    render(<CommandPanel request={{ id: 'fork' }} client={server.client} sessionId="A" current={navigation.capture()} close={close} succeeded={() => {}} opened={() => {}} />);
    const row = await screen.findByRole('option', { name: /Try this/ });
    await act(async () => fireEvent.click(row));
    expect(screen.getByRole('alert').textContent).toContain('Invalid historical Surface revision');
    expect((row as HTMLButtonElement).disabled).toBe(true);
  });
  it('an unavailable historical message cannot be replaced with another displayed boundary', async () => {
    const { navigation } = await subject();
    render(<CommandPanel request={{ id: 'retry', messageId: 'missing-user' }} client={server.client} sessionId="A" current={navigation.capture()} close={() => {}} succeeded={() => {}} opened={() => {}} />);
    expect((await screen.findByRole('alert')).textContent).toContain('not an available native user boundary');
    expect(screen.queryByRole('option')).toBeNull(); expect(methods()).not.toContain('session/branch');
  });
  it.each(['fork', 'branch'] as const)('App navigation after committed %s cannot be redirected by a late reply', async action => {
    const { fixture } = await subject();
    localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A', 'B'] }));
    render(<App client={server.client} workspaceHost={server.workspaceHost} />);
    const input = screen.getByLabelText('Message');
    fireEvent.change(input, { target: { value: `/${action}` } });
    await act(async () => fireEvent.keyDown(input, { key: 'Enter' }));
    const row = await screen.findByRole('option', { name: /Try this/ });
    const method = action === 'fork' ? 'session/fork' : 'session/branch'; server.held.add(method);
    fireEvent.click(row);
    const request = await server.waitFor(method, 1), response = server.commit(request);
    expect(fixture.committed).toHaveLength(1);
    fireEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session B' })));
    const attachments = methods().filter(method => method === 'session/attach').length;
    await act(async () => server.socket.deliver(response));
    expect(screen.getByRole('button', { name: 'Open Session B', current: 'page' })).toBeTruthy();
    expect(methods().filter(method => method === 'session/attach')).toHaveLength(attachments);
    expect(methods()).not.toContain('session/switchNode');
    expect(fixture.committed).toHaveLength(1);
  });
});
