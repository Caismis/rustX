import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { App } from '../src/app/App';
import { Server, applicationReceipt, endpoint, TOKEN } from './fixture';
import { RpcFailure } from '../src/client/errors';
import { cfg3Application } from './cfg3-data';
const key = 'rustx-console-view-v3';
let server: Server;
function newServer() {
  const s = new Server();
  s.handlers.set('workflow/list', request => {
    if (request.method !== 'workflow/list') throw new Error('Expected Workflow library read');
    return {type:'workflow_library',library:{target:request.params.target,directory:'/workflows',absent_revision:'empty',files:[],diagnostic:null}};
  });
  return s;
}
afterEach(() => { cleanup(); server?.client.disconnect(); localStorage.clear(); });
const title = () => { const selected = document.querySelector('#session-title'); return selected?.closest('[hidden]') ? undefined : selected?.textContent; };
const hints = () => JSON.parse(localStorage.getItem(key)!);
const seed = (route: object, openViews = ['A', 'B'], savedEndpoint = endpoint) => localStorage.setItem(key, JSON.stringify({ endpoint: savedEndpoint, openViews, route }));
const mount = () => act(async () => render(<App client={server.client} workspaceHost={server.workspaceHost}/>));
const click = (ui: Awaited<ReturnType<typeof mount>>, name: string) => act(async () => fireEvent.click(ui.getAllByRole('button', { name })[0]));
it.each(['same client', 'new client'] as const)('reload restores selected B with %s while openViews keeps insertion order', async mode => {
  server = newServer(); await server.attached('A', 'B');
  const ui = await mount();
  await click(ui, 'Open Session A'); await click(ui, 'Open Session B');
  expect(title()).toBe('Session B');
  expect(hints()).toEqual({endpoint,openViews:['A','B'],route:{kind:'session',sessionId:'B'}});
  await act(async () => ui.unmount());
  if (mode === 'new client') { await server.client.disconnect(); server = newServer(); }
  await mount();
  if (mode === 'new client') await act(async () => server.connect());
  expect(title()).toBe('Session B');
});
it.each(['workflows', 'schedules', 'new-conversation'] as const)('reload preserves the %s page', async kind => {
  server = newServer(); await server.attached('A', 'B');
  const ui = await mount(); await click(ui, 'Open Session A');
  await click(ui, kind === 'workflows' ? 'Workflows' : kind === 'schedules' ? 'Scheduled Tasks' : 'New Conversation');
  expect(hints().route).toEqual(kind === 'new-conversation' ? { kind, workspaceId: 'workspace-a' } : { kind });
  await act(async () => ui.unmount()); await server.client.disconnect(); server = newServer();
  await mount(); await act(async () => server.connect());
  expect(kind === 'workflows' ? document.querySelector('[data-workflows-level]') : document.querySelector(`[data-shell-route="${kind}"]`)).not.toBeNull();
  expect(title()).toBeUndefined();
});
it('cold startup cannot overwrite its saved selection with the initial new conversation', async () => {
  seed({kind:'session',sessionId:'B'}); server = newServer();
  await mount(); expect(hints().route).toEqual({kind:'session',sessionId:'B'});
  await act(async () => server.connect()); expect(title()).toBe('Session B');
});
it('a pending startup Attach retains the last committed hint until it publishes', async () => {
  seed({kind:'session',sessionId:'B'}); server = newServer(); server.held.add('thread/resume');
  await mount(); await act(async () => server.connect());
  const pending = await server.waitFor('thread/resume', 1);
  expect(pending.params).toMatchObject({threadId:'conversation-B'});
  expect(document.querySelector('[data-shell-route="session-draft"]')).not.toBeNull();
  expect(hints().route).toEqual({kind:'session',sessionId:'B'});
  await act(async () => server.reply(pending));
  expect(title()).toBe('Session B');
  expect(hints().route).toEqual({kind:'session',sessionId:'B'});
});
it('an explicit cold Session selection still saves a safe new conversation while pending', async () => {
  seed({kind:'session',sessionId:'A'}, ['A']); server = newServer(); await server.attached('A');
  const ui = await mount(); server.held.add('thread/resume');
  await click(ui, 'Open Session B'); await server.waitFor('thread/resume', 2);
  expect(document.querySelector('[data-shell-route="session-draft"]')).not.toBeNull();
  expect(hints().route).toEqual({kind:'new-conversation'});
});
it.each(['Workflows', 'New Conversation'] as const)('a user choosing %s before connection supersedes startup restoration', async name => {
  seed({kind:'session',sessionId:'B'}); server = newServer();
  const ui = await mount();
  await click(ui, name); await act(async () => server.connect());
  expect(title()).toBeUndefined();
  expect(hints().route).toEqual(name === 'Workflows' ? { kind: 'workflows' } : { kind: 'new-conversation', workspaceId: 'workspace-a' });
  expect(server.requests.some(row => row.request.method === 'thread/resume' && row.request.params.threadId === 'conversation-B')).toBe(false);
});
it('a later user selection owns navigation after the restored Attach completes', async () => {
  seed({kind:'session',sessionId:'B'}); server = newServer(); server.held.add('thread/resume');
  const ui = await mount(); await act(async () => server.connect());
  const pending = await server.waitFor('thread/resume', 1);
  expect(hints().route).toEqual({kind:'session',sessionId:'B'});
  await click(ui, 'Workflows');
  expect(hints().route).toEqual({kind:'workflows'});
  await act(async () => server.reply(pending));
  expect(document.querySelector('[data-workflows-level]')).not.toBeNull();
  expect(hints().route).toEqual({kind:'workflows'});
});
it('another endpoint never restores a saved route or view membership', async () => {
  seed({kind:'session',sessionId:'B'}, ['A','B'], 'wss://other.example/'); server = newServer();
  await mount(); await act(async () => server.connect());
  expect(title()).toBeUndefined();
  expect(hints()).toEqual({endpoint,openViews:[],route:{kind:'new-conversation',workspaceId:'workspace-a'}});
  expect(server.requests.some(row => row.request.method === 'thread/resume')).toBe(false);
});
it('a locally released Session hint never reopens its native claim', async () => {
  server = newServer(); await server.attached('A','B'); await server.client.release('B');
  seed({kind:'session',sessionId:'B'});
  const before = server.requests.filter(row => row.request.method === 'thread/resume').length;
  await mount(); expect(title()).toBeUndefined();
  expect(hints().route).toEqual({kind:'new-conversation',workspaceId:'workspace-a'});
  expect(hints().openViews).toEqual(['A']);
  expect(server.requests.filter(row => row.request.method === 'thread/resume')).toHaveLength(before);
});
it('configuration notifications and same-authority reconnect preserve the selected route hint', async () => {
  server = newServer(); await server.attached('A','B'); const ui = await mount();
  await click(ui,'Open Session B'); const saved = localStorage.getItem(key);
  await act(async () => server.socket.deliver({jsonrpc:'2.0',method:'configuration/changed',params:{application:applicationReceipt({...cfg3Application(),scope:'B'})}}));
  expect(localStorage.getItem(key)).toBe(saved); expect(title()).toBe('Session B');
  await act(async () => server.client.disconnect()); expect(localStorage.getItem(key)).toBe(saved);
  await act(async () => server.connect()); expect(hints().route).toEqual({kind:'session',sessionId:'B'}); expect(title()).toBe('Session B');
});
it('obsolete v2 membership never supplies a selected route', async () => {
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({endpoint,openViews:['A','B']}));
  server = newServer(); await server.attached('A','B'); await mount(); expect(title()).toBeUndefined();
  expect(hints().route).toEqual({kind:'new-conversation',workspaceId:'workspace-a'});
});
it('only Native unknown_session removes a deleted saved identity without choosing another Session', async () => {
  seed({kind:'session',sessionId:'deleted'}, ['A','deleted']); server = newServer();
  await mount(); await act(async () => server.connect());
  expect(title()).toBeUndefined();
  expect(document.querySelector('[data-shell-route="new-conversation"]')).not.toBeNull();
  expect(hints()).toEqual({endpoint,openViews:['A'],route:{kind:'new-conversation',workspaceId:'workspace-a'}});
});
it.each(['Workflows', 'Open Session A'] as const)('an old unknown_session completion cannot override newer %s intent', async name => {
  seed({kind:'session',sessionId:'deleted'}, ['A','deleted']); server = newServer(); server.held.add('session/read');
  const ui = await mount(); await act(async () => server.connect());
  const pending = await server.waitFor('session/read', 1);
  await click(ui, name);
  // The old restore no longer owns publication when Native rejects its read.
  await act(async () => server.reply(pending));
  if (name === 'Open Session A') {
    server.held.delete('session/read');
    const selected = await server.waitFor('session/read', 2);
    await act(async () => server.reply(selected));
    expect(title()).toBe('Session A');
    expect(hints().route).toEqual({kind:'session',sessionId:'A'});
  } else {
    expect(document.querySelector('[data-workflows-level]')).not.toBeNull();
    expect(hints().route).toEqual({kind:'workflows'});
  }
});
it.each(['authorization', 'operation'] as const)('a restored %s error retains its diagnostic and view membership', async kind => {
  seed({kind:'session',sessionId:'B'}); server = newServer();
  server.handlers.set('session/read', () => { throw new RpcFailure({code:kind === 'authorization' ? -32001 : -32000,message:'Native refused restore',...(kind === 'operation' ? {data:{kind:'operation_failed' as const}} : {})}); });
  const ui = await mount(); await act(async () => server.connect());
  expect(ui.getByRole('alert').textContent).toContain('Native refused restore');
  expect(hints().openViews).toContain('B');
  expect(server.client.getSnapshot().views.B.attachmentIntent).toBe('wanted');
});
it('reload retries the unresolved startup selection after a transient failure and closing another view', async () => {
  seed({kind:'session',sessionId:'B'}); server = newServer(); server.held.add('session/read');
  const ui = await mount(); await act(async () => server.connect());
  const pending = await server.waitFor('session/read', 1);
  expect(pending.params).toMatchObject({session_id:'B'});
  await act(async () => server.socket.deliver({jsonrpc:'2.0',id:pending.id,error:{code:-32000,message:'B temporarily unavailable',data:{kind:'operation_failed'}}}));
  const other = await server.waitFor('session/read', 2);
  await act(async () => server.reply(other));
  await click(ui, 'View options');
  await act(async () => fireEvent.click(ui.getByRole('menuitem', {name:'Close view'})));
  await act(async () => fireEvent.click(ui.getByRole('menuitem', {name:'Session A'})));
  expect(hints()).toEqual({endpoint,openViews:['B'],route:{kind:'session',sessionId:'B'}});
  await act(async () => ui.unmount()); await server.client.disconnect(); server = newServer();
  await mount(); await act(async () => server.connect());
  expect(server.requests.some(row => row.request.method === 'session/read' && row.request.params.session_id === 'B')).toBe(true);
  expect(title()).toBe('Session B');
});
it.each(['Workflows', 'New Conversation', 'Open Session A'] as const)('a failed startup restore yields its saved hint to explicit %s intent', async name => {
  seed({kind:'session',sessionId:'B'}); server = newServer(); server.held.add('session/read');
  const ui = await mount(); await act(async () => server.connect());
  const pending = await server.waitFor('session/read', 1);
  await act(async () => server.socket.deliver({jsonrpc:'2.0',id:pending.id,error:{code:-32000,message:'B temporarily unavailable',data:{kind:'operation_failed'}}}));
  const other = await server.waitFor('session/read', 2);
  server.held.delete('session/read'); await act(async () => server.reply(other));
  await click(ui, name);
  expect(hints().route).toEqual(name === 'Open Session A' ? {kind:'session',sessionId:'A'} : name === 'Workflows' ? {kind:'workflows'} : {kind:'new-conversation',workspaceId:'workspace-a'});
});
it('a failed explicit Fork retires the unresolved startup hint even when its draft stays visible', async () => {
  seed({kind:'session',sessionId:'B'}); server = newServer(); server.held.add('session/read');
  const ui = await mount(); await act(async () => server.connect());
  const pending = await server.waitFor('session/read', 1);
  await act(async () => server.socket.deliver({jsonrpc:'2.0',id:pending.id,error:{code:-32000,message:'B temporarily unavailable',data:{kind:'operation_failed'}}}));
  const other = await server.waitFor('session/read', 2);
  server.held.delete('session/read'); await act(async () => server.reply(other));
  expect(hints().route).toEqual({kind:'session',sessionId:'B'});
  await click(ui, 'Session actions for Session A');
  await act(async () => fireEvent.click(ui.getByRole('menuitem', {name:'Fork session'})));
  expect(ui.getByRole('alert').textContent).toContain('completed response');
  expect(document.querySelector('[data-shell-route="session-draft"]')).not.toBeNull();
  expect(hints().route).toEqual({kind:'new-conversation'});
});
it('a stale transient restore failure cannot replace a newer page or its saved hint', async () => {
  seed({kind:'session',sessionId:'B'}); server = newServer(); server.held.add('session/read');
  const ui = await mount(); await act(async () => server.connect());
  const pending = await server.waitFor('session/read', 1);
  await click(ui, 'Workflows');
  await act(async () => server.socket.deliver({jsonrpc:'2.0',id:pending.id,error:{code:-32000,message:'Obsolete restore failure',data:{kind:'operation_failed'}}}));
  expect(document.querySelector('[data-workflows-level]')).not.toBeNull();
  expect(hints().route).toEqual({kind:'workflows'});
  expect(ui.queryByText('Obsolete restore failure')).toBeNull();
});

it('a Home round trip keeps a cold explicit selection ephemeral until Native admits it', async () => {
  seed({ kind: 'session', sessionId: 'A' }, ['A']);
  server = newServer(); await server.attached('A');
  const ui = await mount();
  server.held.add('thread/resume');
  await click(ui, 'Open Session B');
  const pending = await server.waitFor('thread/resume', 2);
  expect(hints().route).toEqual({ kind: 'new-conversation' });
  await click(ui, 'Workflows');
  await click(ui, 'Home');
  expect(document.querySelector('[data-shell-route="session-draft"]')).not.toBeNull();
  expect(hints().route).toEqual({ kind: 'new-conversation' });
  await act(async () => server.reply(pending));
  await waitFor(() => expect(hints().route).toEqual({ kind: 'session', sessionId: 'B' }));
  expect(title()).toBe('Session B');
});

it.each([false, true])('Connection Settings preserves startup restoration while newer explicit navigation wins: %s', async newerNavigation => {
  seed({ kind: 'session', sessionId: 'B' }); server = newServer();
  const ui = await mount();
  if (newerNavigation) await click(ui, 'Workflows');
  await click(ui, 'Settings');
  await act(async () => fireEvent.click(ui.getByRole('tab', { name: 'Advanced' })));
  await click(ui, 'Connection');
  await act(async () => fireEvent.change(ui.getByLabelText('Connection mode', { exact: true }), { target: { value: 'remote' } }));
  fireEvent.change(ui.getByLabelText('WebSocket endpoint', { exact: true }), { target: { value: endpoint } });
  fireEvent.change(ui.getByLabelText('Transport token', { exact: true }), { target: { value: TOKEN } });
  await click(ui, 'Connect');
  expect(server.client.getSnapshot().connection).toBe('connected');
  await click(ui, 'Home');
  if (newerNavigation) {
    expect(title()).toBeUndefined();
    expect(hints().route).toEqual({ kind: 'new-conversation', workspaceId: 'workspace-a' });
    expect(server.requests.some(row => row.request.method === 'thread/resume' && row.request.params.threadId === 'conversation-B')).toBe(false);
  } else {
    await waitFor(() => expect(title()).toBe('Session B'));
    expect(hints().route).toEqual({ kind: 'session', sessionId: 'B' });
  }
});

it('a committed Session stays selected across a disconnected Home return and a fresh page', async () => {
  server = newServer(); await server.attached('A', 'B');
  const ui = await mount(); await click(ui, 'Open Session B');
  await click(ui, 'Settings');
  await act(async () => server.client.disconnect());
  await click(ui, 'Home');
  expect(title()).toBe('Session B');
  expect(document.querySelector('[data-shell-route="session-draft"]')).not.toBeNull();
  expect(hints().route).toEqual({ kind: 'session', sessionId: 'B' });
  await act(async () => server.connect());
  await waitFor(() => expect(document.querySelector('[data-shell-route="session"]')).not.toBeNull());
  expect(hints().route).toEqual({ kind: 'session', sessionId: 'B' });
  await act(async () => ui.unmount()); await server.client.disconnect(); server = newServer();
  await mount(); await act(async () => server.connect());
  await waitFor(() => expect(title()).toBe('Session B'));
});

it('a failed startup hint survives Settings while its same Home return awaits authorization', async () => {
  seed({ kind: 'session', sessionId: 'B' }); server = newServer(); server.held.add('session/read');
  const ui = await mount(); await act(async () => server.connect());
  const pending = await server.waitFor('session/read', 1);
  await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: pending.id, error: { code: -32000, message: 'B temporarily unavailable', data: { kind: 'operation_failed' } } }));
  const other = await server.waitFor('session/read', 2);
  server.held.delete('session/read'); await act(async () => server.reply(other));
  await click(ui, 'Settings');
  expect(hints().route).toEqual({ kind: 'session', sessionId: 'B' });
  let release!: () => void, entered!: () => void;
  const authorization = new Promise<void>(resolve => { release = resolve; });
  const classifying = new Promise<void>(resolve => { entered = resolve; });
  const classify = server.workspaceHost.classifyLocations;
  server.workspaceHost.classifyLocations = async (...args) => { entered(); await authorization; return classify(...args); };
  const before = server.requests.length;
  try {
    await click(ui, 'Home'); await classifying;
    expect(ui.getByRole('region', { name: 'Settings' })).toBeTruthy();
    expect(hints().route).toEqual({ kind: 'session', sessionId: 'B' });
    expect(server.requests.slice(before).filter(row => ['thread/start', 'session/submissionIntent', 'inbound/queue', 'turn/start'].includes(row.request.method))).toEqual([]);
  } finally {
    await act(async () => release());
  }
  expect(hints().route).toEqual({ kind: 'session', sessionId: 'B' });
});
