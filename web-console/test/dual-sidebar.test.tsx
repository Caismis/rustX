import { act, cleanup, fireEvent, render, screen, waitFor as waitForUI, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import { waitFor } from 'xstate';
import { App } from '../src/app/App';
import { draftSessionCurrent, isHomeRoute, type ShellNavigationActor, type ShellNavigationEvent } from '../src/app/navigation/machine';
import { userSettingsTarget, workspaceSettingsTarget } from '../src/app/settings/projection';
import { setWorkflow } from '../src/app/workflows/document/commands';
import type { WorkflowScopePort } from '../src/app/workflows/port';
import { Server, boundedSettings, snapshot } from './fixture';
import { cfg3Source } from './cfg3-data';
import { routeFixture } from './shell-navigation-fixture';
import { statusSource } from './workflow-fixtures';

let server: Server;
beforeEach(() => {
  localStorage.clear(); server = new Server();
  server.handlers.set('workflow/list', () => ({ type: 'workflow_library', library: { target: { kind: 'user' }, directory: '/workflows', absent_revision: 'missing', diagnostic: null, files: [] } }));
});
afterEach(() => { cleanup(); server.client.disconnect(); });
const idle = (actor: ShellNavigationActor) => waitFor(actor, snapshot => snapshot.matches('idle'));
const executableRequests = (at = 0) => server.requests.slice(at).filter(({ request }) => /^(turn\/|thread\/(start|resume|fork)$|inbound\/(queue|edit|remove)$|session\/(detach|switchNode|submissionIntent|uploadPrepare)$|workflow\/(start|cancel)|schedule\/(create|cancel|update))/.test(request.method));
async function mounted(session = true) {
  await server.attached('A');
  render(<App client={server.client} workspaceHost={server.workspaceHost}/>);
  if (session) await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
}
const primary = (name: string) => within(screen.getByRole('navigation', { name: 'Primary navigation' })).getByRole('button', { name });

it('Home is the only secondary sidebar, active Home expands it without replacing its conversation', async () => {
  await mounted();
  const text = screen.getByRole('textbox', { name: 'Message' });
  fireEvent.change(text, { target: { value: 'Retain this logical editor' } });
  const incarnation = document.querySelector('[data-editor-incarnation]')?.getAttribute('data-editor-incarnation');
  const at = server.requests.length;
  expect(primary('Home').getAttribute('aria-current')).toBe('page');
  screen.getByRole('button', { name: 'Collapse Sidebar' }).focus();
  fireEvent.click(screen.getByRole('button', { name: 'Collapse Sidebar' }));
  expect(document.activeElement).toBe(primary('Home'));
  expect(screen.queryByRole('button', { name: 'New Conversation' })).toBeNull();
  expect(document.querySelector('#home-sidebar')?.hasAttribute('hidden')).toBe(true);
  fireEvent.click(primary('Home')); fireEvent.click(primary('Home'));
  expect(document.querySelector('#home-sidebar')?.hasAttribute('hidden')).toBe(false);
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(text);
  expect((text as HTMLTextAreaElement).value).toBe('Retain this logical editor');
  expect(document.querySelector('[data-editor-incarnation]')?.getAttribute('data-editor-incarnation')).toBe(incarnation);
  expect(executableRequests(at)).toEqual([]);
});

it.each(['Workflows', 'Scheduled Tasks', 'Settings'])('%s is an independent page and Home restores Session, text, files and editor identity', async name => {
  await mounted();
  const editor = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
  fireEvent.change(editor, { target: { value: 'Unsent Session draft' } });
  await act(async () => fireEvent.change(screen.getByLabelText('Attach files'), { target: { files: [new File(['retained'], 'retained.txt')] } }));
  await waitForUI(() => expect(document.querySelector('[data-attachment-state=ready]')).toBeTruthy());
  const incarnation = document.querySelector('[data-editor-incarnation]')?.getAttribute('data-editor-incarnation');
  const target = server.client.target('A'), at = server.requests.length;
  await act(async () => fireEvent.click(primary(name)));
  expect(primary(name).getAttribute('aria-current')).toBe('page');
  expect(screen.queryByRole('button', { name: 'New Conversation' })).toBeNull();
  expect(screen.queryByRole('tree', { name: 'Session browser' })).toBeNull();
  expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull();
  expect(screen.getByRole('navigation', { name: 'Primary navigation' }).querySelectorAll('[aria-current=page]')).toHaveLength(1);
  expect(document.querySelector('[data-rightbar-col]')?.hasAttribute('hidden')).toBe(true);
  if (name === 'Settings') {
    expect(screen.getByRole('region', { name: 'Settings' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Close Settings' })).toBeNull();
    fireEvent.keyDown(screen.getByRole('region', { name: 'Settings' }), { key: 'Escape' });
    expect(primary('Settings').getAttribute('aria-current')).toBe('page');
  }
  await act(async () => fireEvent.click(primary('Home')));
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(editor);
  expect(editor.value).toBe('Unsent Session draft');
  expect(screen.getByText('retained.txt')).toBeTruthy();
  expect(document.querySelector('[data-editor-incarnation]')?.getAttribute('data-editor-incarnation')).toBe(incarnation);
  expect(server.client.target('A')).toBe(target);
  expect(executableRequests(at)).toEqual([]);
});

it('a new conversation keeps its Workspace, local file, text and incarnation across Settings', async () => {
  server.workspaceHost.resolveWorkspace = async () => ({ cwd: '/workspace' });
  await mounted(false);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Choose Workspace' })));
  fireEvent.click(await screen.findByRole('menuitem', { name: 'Workspace A' }));
  const editor = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
  fireEvent.change(editor, { target: { value: 'Draft before any native Session' } });
  await act(async () => fireEvent.change(screen.getByLabelText('Attach files'), { target: { files: [new File(['local'], 'local.txt')] } }));
  const incarnation = document.querySelector('[data-editor-incarnation]')?.getAttribute('data-editor-incarnation');
  const at = server.requests.length;
  await act(async () => fireEvent.click(primary('Settings')));
  await act(async () => fireEvent.click(primary('Home')));
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(editor);
  expect(editor.value).toBe('Draft before any native Session');
  expect(screen.getByText('local.txt')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Choose Workspace' }).textContent).toContain('Workspace A');
  expect(document.querySelector('[data-editor-incarnation]')?.getAttribute('data-editor-incarnation')).toBe(incarnation);
  expect(executableRequests(at)).toEqual([]);
});

it('primary Settings restores its exact internal target, section and detail', async () => {
  await server.attached('A');
  const f = routeFixture(server.client, server.workspaceHost);
  f.actor.send({ type: 'SETTINGS', destination: { type: 'OPEN', target: workspaceSettingsTarget('project', 'rustX') } });
  f.settings.send({ type: 'SELECT', page: 'models' });
  f.settings.send({ type: 'FOCUS', focus: { kind: 'provider', id: 'transport' } });
  const internal = f.settings.getSnapshot().context;
  f.actor.send({ type: 'SCHEDULES' }); f.actor.send({ type: 'SETTINGS' });
  expect(f.settings.getSnapshot().context.target).toBe(internal.target);
  expect(f.settings.getSnapshot().context.page).toBe('models');
  expect(f.settings.getSnapshot().context.focus).toBe(internal.focus);
});

it.each(['deleted', 'unauthorized', 'authority replaced'] as const)('Home cannot reopen a remembered target that is %s', async reason => {
  await server.attached('A');
  const host = { ...server.workspaceHost, classifyLocations: async () => [reason === 'unauthorized' ? { authorized: false as const, reason: 'denied' as const } : { authorized: true as const }] };
  const f = routeFixture(server.client, host);
  f.actor.send({ type: 'SCHEDULES' });
  const old = f.capture();
  if (reason === 'deleted') { server.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'deleted', session_id: 'A' } })); await server.client.deleteSession('A', 'revision-A'); }
  if (reason === 'authority replaced') { server.authorityId = 'replacement'; await server.connect(); }
  const at = server.requests.length;
  f.actor.send({ type: 'HOME' }); await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'new-conversation' });
  expect(old()).toBe(false);
  expect(executableRequests(at)).toEqual([]);
});

it('a released native attachment leaves the retained Home editor local without reopening its Session', async () => {
  await server.attached('A');
  const f = routeFixture(server.client, server.workspaceHost);
  const binding = f.actor.getSnapshot().context.binding;
  f.actor.send({ type: 'SETTINGS' });
  await server.client.release('A');
  const at = server.requests.length;
  f.actor.send({ type: 'HOME' }); await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session-draft', sessionId: 'A' });
  expect(f.actor.getSnapshot().context.binding).toBe(binding);
  expect(server.client.isAttachmentControlCurrent('A', server.client.getSnapshot().views.A.attachmentObservation)).toBe(false);
  expect(executableRequests(at)).toEqual([]);
});

it('a superseded Home authorization cannot publish its old destination', async () => {
  await server.attached('A');
  let release!: () => void;
  const held = new Promise<void>(resolve => { release = resolve; });
  const f = routeFixture(server.client, { ...server.workspaceHost, classifyLocations: async () => { await held; return [{ authorized: true }]; } });
  f.actor.send({ type: 'WORKFLOWS' }); f.actor.send({ type: 'HOME' });
  f.actor.send({ type: 'SETTINGS', destination: { type: 'OPEN.CONNECTION' } });
  release(); await held;
  await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'settings' });
  expect(f.settings.getSnapshot().context.focus.advanced).toEqual({ kind: 'connection' });
});

async function dirtyWorkflow() {
  await server.attached('A');
  const f = routeFixture(server.client, server.workspaceHost), source = statusSource();
  const port: WorkflowScopePort = {
    endpoint: 'ws://127.0.0.1:8080/', scope: { kind: 'user' }, read: async () => source,
    list: async () => { throw Error('Not requested'); }, write: async () => { throw Error('Not requested'); },
    check: async () => { throw Error('Not requested'); }, analyze: async () => { throw Error('Not requested'); },
    modelChoices: async () => { throw Error('Not requested'); }, toolCatalog: async () => ({ sources: [] }), enable: async () => { throw Error('Not requested'); },
  };
  f.actor.send({ type: 'WORKFLOWS' });
  f.workflows.send({ type: 'OPEN', id: source.id, port });
  const editor = f.workflows.getSnapshot().context.editor!;
  await waitFor(editor, snapshot => snapshot.matches('ready'));
  editor.send({ type: 'EDIT', apply: document => setWorkflow(document, { description: 'unsaved', timeout_ms: document.timeout_ms }) });
  return { ...f, editor };
}
const exits: ShellNavigationEvent[] = [
  { type: 'HOME' }, { type: 'SCHEDULES' }, { type: 'SETTINGS', destination: { type: 'OPEN', target: userSettingsTarget } },
  { type: 'OPEN', destination: { kind: 'session', sessionId: 'A', sidebar: false } }, { type: 'DRAFT' },
];
it.each(exits)('dirty Workflows guards $type and cancellation preserves the committed route', async event => {
  const f = await dirtyWorkflow();
  f.actor.send(event);
  expect(f.workflows.getSnapshot().matches('confirming')).toBe(true);
  expect(f.actor.getSnapshot().context.route.kind).toBe('workflows');
  f.workflows.send({ type: 'CANCEL' });
  expect(f.workflows.getSnapshot().context.editor).toBe(f.editor);
  expect(f.editor.getSnapshot().context.document?.description).toBe('unsaved');
  expect(f.actor.getSnapshot().context.route.kind).toBe('workflows');
});
it('replacement Workflow leave gestures fence obsolete callbacks and commit the latest destination exactly once', async () => {
  const f = await dirtyWorkflow();
  f.actor.send({ type: 'HOME' });
  const obsolete = f.workflows.getSnapshot().context.leaving!;
  f.actor.send({ type: 'SETTINGS', destination: { type: 'OPEN.CONNECTION' } });
  if (obsolete.kind !== 'external') throw Error('Expected external leave');
  obsolete.proceed();
  expect(f.actor.getSnapshot().context.route.kind).toBe('workflows');
  let commits = 0, last = f.actor.getSnapshot().context.route;
  const subscription = f.actor.subscribe(snapshot => { if (snapshot.context.route !== last) { last = snapshot.context.route; if (last.kind === 'settings') commits++; } });
  f.workflows.send({ type: 'CONFIRM' });
  expect(f.actor.getSnapshot().context.route.kind).toBe('settings');
  expect(f.settings.getSnapshot().context.focus.advanced).toEqual({ kind: 'connection' });
  expect(commits).toBe(1);
  obsolete.proceed();
  expect(commits).toBe(1);
  expect(isHomeRoute(f.actor.getSnapshot().context.route)).toBe(false);
  subscription.unsubscribe();
});

it('cancelling a Workflow leave retires its callback even without a replacement gesture', async () => {
  const f = await dirtyWorkflow();
  f.actor.send({ type: 'HOME' });
  const obsolete = f.workflows.getSnapshot().context.leaving!;
  f.workflows.send({ type: 'CANCEL' });
  if (obsolete.kind !== 'external') throw Error('Expected external leave');
  obsolete.proceed();
  expect(f.actor.getSnapshot().context.route.kind).toBe('workflows');
  expect(f.workflows.getSnapshot().context.editor).toBe(f.editor);
});

it('Settings drafts and a held exact-CAS write survive primary navigation without replay or a new read', async () => {
  const source = cfg3Source();
  server.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: boundedSettings(source) }));
  server.handlers.set('configuration/sourceWrite', request => {
    if (request.method !== 'configuration/sourceWrite' || request.params.mutation.kind !== 'config' || request.params.mutation.mutation.unit !== 'provider') throw Error('Expected exact Provider write');
    source.user.authored!.providers!.transport.base_url = request.params.mutation.mutation.authored!.base_url;
    source.user.revision = 'user-2';
    return { type: 'sources_written', revision: 'user-2', configuration_revision: '1' };
  });
  await mounted();
  await act(async () => fireEvent.click(primary('Settings')));
  await act(async () => fireEvent.click(screen.getByRole('tab', { name: 'Models' })));
  await act(async () => fireEvent.click(screen.getByRole('row', { name: 'transport' })));
  const field = screen.getByLabelText('Base URL') as HTMLInputElement;
  fireEvent.change(field, { target: { value: 'https://retained.example/' } });
  const reads = server.requests.filter(row => row.request.method === 'configuration/sourcesRead').length;
  await act(async () => fireEvent.click(primary('Home')));
  await act(async () => fireEvent.click(primary('Settings')));
  expect(screen.getByLabelText('Base URL')).toBe(field);
  expect(field.value).toBe('https://retained.example/');
  expect(server.requests.filter(row => row.request.method === 'configuration/sourcesRead')).toHaveLength(reads);
  server.held.add('configuration/sourceWrite');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save Provider transport' })));
  await waitForUI(() => expect(server.requests.some(row => row.request.method === 'configuration/sourceWrite')).toBe(true));
  const write = server.requests.find(row => row.request.method === 'configuration/sourceWrite')!.request;
  expect(write.params).toMatchObject({ expected_revision: 'user-1', mutation: { kind: 'config', mutation: { unit: 'provider', id: 'transport', authored: { base_url: 'https://retained.example/' } } } });
  await act(async () => fireEvent.click(primary('Home')));
  await act(async () => server.reply(write));
  await act(async () => fireEvent.click(primary('Settings')));
  expect((screen.getByLabelText('Base URL') as HTMLInputElement).value).toBe('https://retained.example/');
  expect(server.requests.filter(row => row.request.method === 'configuration/sourceWrite')).toHaveLength(1);
  expect((screen.getByRole('button', { name: 'Save Provider transport' }) as HTMLButtonElement).disabled).toBe(true);
});

it('returning Home while a cold attachment is pending keeps its draft and waits for the original lifecycle Open', async () => {
  server.snapshots.set('C', snapshot('C'));
  await server.attached('A');
  const f = routeFixture(server.client, { ...server.workspaceHost, classifyLocations: async cwds => cwds.map(() => ({ authorized: true })) });
  server.held.add('thread/resume');
  f.actor.send({ type: 'OPEN', destination: { kind: 'session', sessionId: 'C', sidebar: true } });
  const attach = await server.waitFor('thread/resume', 2), binding = f.actor.getSnapshot().context.binding;
  const current = draftSessionCurrent(f.actor, f.actor.getSnapshot().context.draftSession!.intent);
  const retainedOwner = f.capture();
  f.actor.send({ type: 'SETTINGS' });
  expect(current()).toBe(true);
  f.actor.send({ type: 'HOME' }); await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'session-draft', sessionId: 'C' });
  expect(f.actor.getSnapshot().context.binding).toBe(binding);
  expect(current()).toBe(true);
  expect(draftSessionCurrent(f.actor, f.actor.getSnapshot().context.draftSession!.intent)()).toBe(true);
  server.reply(attach);
  await server.client.waitForAttachment('C', () => true);
  await waitFor(f.actor, state => state.context.route.kind === 'session');
  expect(f.actor.getSnapshot().context.binding).toBe(binding);
  expect(current()).toBe(true);
  expect(retainedOwner()).toBe(true);
  expect(server.requests.filter(row => row.request.method === 'thread/resume' && server.sessionForThread(row.request.params.threadId) === 'C')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method.startsWith('turn/'))).toEqual([]);
  f.actor.send({ type: 'DRAFT' });
  expect(current()).toBe(false);
});

it('refresh hints restore the last Home Session while a different primary page is selected', async () => {
  await server.attached('A', 'B');
  const first = render(<App client={server.client} workspaceHost={server.workspaceHost}/>);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session B' })));
  await act(async () => fireEvent.click(primary('Settings')));
  expect(JSON.parse(localStorage.getItem('rustx-console-view-v3')!)).toEqual({ endpoint: server.client.getSnapshot().endpoint, openViews: ['A', 'B'], route: { kind: 'session', sessionId: 'B' } });
  const at = server.requests.length;
  first.unmount();
  await act(async () => { render(<App client={server.client} workspaceHost={server.workspaceHost}/>); });
  expect(screen.getByLabelText('Session title').textContent).toBe('Session B');
  expect(executableRequests(at)).toEqual([]);
});

it.each(['unavailable', 'denied'] as const)('a registered but %s Workspace cannot restore an unsent Home draft', async failure => {
  await server.attached('A');
  let revoked = false;
  const host = { ...server.workspaceHost,
    resolveWorkspace: async () => { if (revoked && failure === 'unavailable') throw Error('Workspace root unavailable'); return { cwd: '/workspace/A' }; },
    classifyLocations: async () => revoked && failure === 'denied' ? [{ authorized: false as const, reason: 'denied' as const }] : [{ authorized: true as const }],
  };
  const f = routeFixture(server.client, host);
  f.actor.send({ type: 'DRAFT', workspaceId: 'workspace-a' });
  f.actor.send({ type: 'SETTINGS' }); revoked = true;
  const at = server.requests.length;
  f.actor.send({ type: 'HOME' }); await idle(f.actor);
  expect(f.actor.getSnapshot().context.route).toEqual({ kind: 'new-conversation' });
  expect(executableRequests(at)).toEqual([]);
});

it('reselecting active Workflows cancels a pending Home return without leaving its page', async () => {
  await server.attached('A');
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const f = routeFixture(server.client, { ...server.workspaceHost, classifyLocations: async () => { await gate; return [{ authorized: true }]; } });
  f.actor.send({ type: 'WORKFLOWS' }); f.actor.send({ type: 'HOME' });
  const pending = f.actor.getSnapshot().context.pending!;
  f.actor.send({ type: 'WORKFLOWS' });
  release(); await gate;
  await idle(f.actor);
  expect(f.actor.getSnapshot().context.route.kind).toBe('workflows');
  expect(f.actor.getSnapshot().context.pending).toBeUndefined();
  expect(f.actor.getSnapshot().context.publication?.intent).not.toBe(pending);
});
