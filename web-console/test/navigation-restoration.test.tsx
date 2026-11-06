import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { App } from '../src/app/App';
import { Server, applicationReceipt, endpoint } from './fixture';
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
const title = () => document.querySelector('#session-title')?.textContent;
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
  await click(ui, kind === 'workflows' ? 'Workflows' : kind === 'schedules' ? 'Automation tasks' : 'New Conversation');
  expect(hints().route).toEqual({kind});
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
  seed({kind:'session',sessionId:'B'}); server = newServer(); server.held.add('session/attach');
  await mount(); await act(async () => server.connect());
  const pending = await server.waitFor('session/attach', 1);
  expect(pending.params).toMatchObject({session_id:'B'});
  expect(document.querySelector('[data-shell-route="session-draft"]')).not.toBeNull();
  expect(hints().route).toEqual({kind:'session',sessionId:'B'});
  await act(async () => server.reply(pending));
  expect(title()).toBe('Session B');
  expect(hints().route).toEqual({kind:'session',sessionId:'B'});
});
it('an explicit cold Session selection still saves a safe new conversation while pending', async () => {
  seed({kind:'session',sessionId:'A'}, ['A']); server = newServer(); await server.attached('A');
  const ui = await mount(); server.held.add('session/attach');
  await click(ui, 'Open Session B'); await server.waitFor('session/attach', 2);
  expect(document.querySelector('[data-shell-route="session-draft"]')).not.toBeNull();
  expect(hints().route).toEqual({kind:'new-conversation'});
});
it.each(['Workflows', 'New Conversation'] as const)('a user choosing %s before connection supersedes startup restoration', async name => {
  seed({kind:'session',sessionId:'B'}); server = newServer();
  const ui = await mount();
  await click(ui, name); await act(async () => server.connect());
  expect(title()).toBeUndefined();
  expect(hints().route).toEqual({kind:name === 'Workflows' ? 'workflows' : 'new-conversation'});
  expect(server.requests.some(row => row.request.method === 'session/attach' && row.request.params.session_id === 'B')).toBe(false);
});
it('a later user selection owns navigation after the restored Attach completes', async () => {
  seed({kind:'session',sessionId:'B'}); server = newServer(); server.held.add('session/attach');
  const ui = await mount(); await act(async () => server.connect());
  const pending = await server.waitFor('session/attach', 1);
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
  expect(hints()).toEqual({endpoint,openViews:[],route:{kind:'new-conversation'}});
  expect(server.requests.some(row => row.request.method === 'session/attach')).toBe(false);
});
it('a locally released Session hint never reopens its native claim', async () => {
  server = newServer(); await server.attached('A','B'); await server.client.release('B');
  seed({kind:'session',sessionId:'B'});
  const before = server.requests.filter(row => row.request.method === 'session/attach').length;
  await mount(); expect(title()).toBeUndefined();
  expect(hints().route).toEqual({kind:'new-conversation'});
  expect(hints().openViews).toEqual(['A']);
  expect(server.requests.filter(row => row.request.method === 'session/attach')).toHaveLength(before);
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
  expect(hints().route).toEqual({kind:'new-conversation'});
});
it('only Native unknown_session removes a deleted saved identity without choosing another Session', async () => {
  seed({kind:'session',sessionId:'deleted'}, ['A','deleted']); server = newServer();
  await mount(); await act(async () => server.connect());
  expect(title()).toBeUndefined();
  expect(document.querySelector('[data-shell-route="new-conversation"]')).not.toBeNull();
  expect(hints()).toEqual({endpoint,openViews:['A'],route:{kind:'new-conversation'}});
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
