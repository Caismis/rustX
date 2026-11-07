import {threadFor} from '../../protocol/app-server/testing.ts';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import type { Request } from '../../protocol/app-server/v74';
import { App } from '../src/app/App';
import { firstSubmissions } from '../src/app/new-conversation/first-submit';
import { Server, boundedSettings, snapshot } from './fixture';
import { cfg3Effective, cfg3Source } from './cfg3-data';

let server: Server;
beforeEach(() => {
  localStorage.clear();
  server = new Server();
  server.workspaceHost.resolveWorkspace = async () => ({ cwd: '/workspace/A' });
  server.handlers.set('workflow/list', () => ({ type: 'workflow_library', library: { target: { kind: 'user' }, directory: '/workflows', absent_revision: 'missing', diagnostic: null, files: [] } }));
});
afterEach(() => { cleanup(); server.client.disconnect(); });
const primary = (name: string) => within(screen.getByRole('navigation', { name: 'Primary navigation' })).getByRole('button', { name });
async function navigate(name: string) {
  const control = primary(name);
  await act(async () => fireEvent.click(control));
  await waitFor(() => expect(control.getAttribute('aria-current')).toBe('page'));
}
async function mounted(selected = true) {
  await server.attached('A');
  render(<App client={server.client} workspaceHost={server.workspaceHost}/>);
  if (selected) await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session A' })));
}

it('an uncertain Attach across Settings retains the selected local editor without another native operation', async () => {
  await mounted();
  server.held.add('thread/resume');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session B' })));
  const attach = await server.waitFor('thread/resume', 2), carrier = server.socket;
  const editor = screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement;
  fireEvent.change(editor, { target: { value: 'Keep this while Attach needs verification' } });
  await act(async () => fireEvent.change(screen.getByLabelText('Attach files'), { target: { files: [new File(['local'], 'uncertain.txt')] } }));
  const incarnation = document.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation');
  await navigate('Settings');
  await act(async () => { server.commit(attach, carrier); carrier.close(); });
  await waitFor(() => expect(server.client.getSnapshot().views.B.attachmentIntent).toBe('released'));
  const at = server.requests.length;
  await navigate('Home');
  expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe('session-draft');
  expect(screen.getByRole('textbox', { name: 'Message' })).toBe(editor);
  expect(editor.value).toBe('Keep this while Attach needs verification');
  expect(screen.getByText('uncertain.txt')).toBeTruthy();
  expect(document.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation')).toBe(incarnation);
  expect(server.requests.slice(at).filter(({ request }) => /^(turn\/|thread\/(start|resume|fork)$|inbound\/(queue|edit|remove)$|session\/(detach|submissionIntent|uploadPrepare)$)/.test(request.method))).toEqual([]);
});

async function firstSend(hold?: 'thread/resume' | 'session/subscribe', file?: File) {
  const capabilities = { inputModalities: ['text' as const], outputModalities: ['text' as const], jsonSchemaOutput: false };
  server.workspaceHost.configureWorkspace = async () => ({ kind: 'read', projection: {
    ...cfg3Source(), target: { kind: 'workspace', directory: '/workspace/A' }, prospective_approval_mode: 'policy',
    session_models: { kind: 'available', default_model: { model: 'main' }, catalog: { models: [{ model: 'main', protocol: 'openai_responses', contextWindow: 128000, maxOutputTokens: 8192, credentialSource: { type: 'environment', variable: 'KEY' }, declaredCapabilities: capabilities, effectiveCapabilities: capabilities, profiles: [] }] } },
  } });
  const session = { id: 'created', active_node: 'node-created', active_conversation_id: 'conversation-created', node_count: 1, created_at: '0', updated_at: '0' };
  server.handlers.set('thread/start', () => ({thread:threadFor(session), type: 'session_transition', session }));
  server.summaries.set(session.id, { cwd: '/workspace/A' });
  server.snapshots.set(session.id, snapshot(session.id));
  server.nodeSnapshots.set(session.active_node, server.snapshots.get(session.id)!);
  server.held.add('thread/start');
  await mounted(false);
  if (hold) server.held.add(hold);
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Choose Workspace' })));
  await act(async () => fireEvent.click(await screen.findByRole('menuitem', { name: 'Workspace A' })));
  fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'One original submission' } });
  if (file) await act(async () => fireEvent.change(screen.getByLabelText('Attach files'), { target: { files: [file] } }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Send' })).toHaveProperty('disabled', false));
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
  const create = await server.waitFor('thread/start', 1);
  const incarnation = document.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation');
  return { session, create, incarnation };
}

async function holdHomeAuthorization() {
  const classify = server.workspaceHost.classifyLocations;
  let entered!: () => void, release!: () => void;
  const reached = new Promise<void>(resolve => { entered = resolve; });
  const held = new Promise<void>(resolve => { release = resolve; });
  let first = true;
  server.workspaceHost.classifyLocations = async (...args) => {
    if (first) { first = false; entered(); await held; }
    return classify(...args);
  };
  await act(async () => fireEvent.click(primary('Home')));
  await reached;
  return release;
}

const creationReturns = ['Settings', 'Workflows', 'Scheduled Tasks'].flatMap(page =>
  ['published Home', 'authorizing Home', 'newer Session'].map(arrival => ({ page, arrival })));
it.each(creationReturns)('the first creation ACK from $page follows only its retained editor at $arrival', async ({ page, arrival }) => {
  const { session, create, incarnation } = await firstSend();
  await navigate(page);
  let releaseHome: (() => void) | undefined;
  if (arrival === 'authorizing Home') {
    releaseHome = await holdHomeAuthorization();
  } else {
    await navigate('Home');
    if (arrival === 'newer Session') await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Open Session B' })));
  }
  await act(async () => server.reply(create));
  await waitFor(() => expect(firstSubmissions(server.client).session(session.id)?.phase).toBe(arrival === 'newer Session' ? 'failed' : 'admitted'));
  if (releaseHome) await act(async () => releaseHome!());
  await waitFor(() => expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe('session'));
  if (arrival !== 'newer Session') expect(document.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation')).toBe(incarnation);
  expect(screen.getByLabelText('Session title').textContent).toBe(arrival === 'newer Session' ? 'Session B' : 'Session created');
  expect(server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'thread/resume' && server.sessionForThread(row.request.params.threadId) === session.id)).toHaveLength(arrival === 'newer Session' ? 0 : 1);
  expect(server.requests.filter(row => row.request.method === 'inbound/queue')).toHaveLength(arrival === 'newer Session' ? 0 : 1);
});

const attachingReturns = ['Settings', 'Workflows', 'Scheduled Tasks'].flatMap(page =>
  (['thread/resume', 'session/subscribe'] as const).map(hold => ({ page, hold })));
it.each(attachingReturns)('a known creation from $page retains its original Send while $hold is held across Home return', async ({ page, hold }) => {
  const { session, create, incarnation } = await firstSend(hold);
  await navigate(page);
  await act(async () => server.reply(create));
  const attachment = await server.waitFor(hold, 2);
  await navigate('Home');
  expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe('session-draft');
  await act(async () => { server.held.delete(hold); server.reply(attachment); });
  await waitFor(() => expect(firstSubmissions(server.client).session(session.id)?.phase).toBe('admitted'));
  await waitFor(() => expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe('session'));
  expect(document.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation')).toBe(incarnation);
  expect(server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'thread/resume' && server.sessionForThread(row.request.params.threadId) === session.id)).toHaveLength(1);
  expect(server.requests.filter(row => row.request.method === 'inbound/queue')).toHaveLength(1);
});

it.each(['Settings', 'Workflows', 'Scheduled Tasks'])('an admitted creation from %s follows native readiness after Home authorization resumes during resync', async page => {
  const { session, create, incarnation } = await firstSend();
  await navigate(page);
  const releaseHome = await holdHomeAuthorization();
  let updates = 0;
  let stop = () => {};
  try {
    await act(async () => server.reply(create));
    await waitFor(() => expect(firstSubmissions(server.client).session(session.id)?.phase).toBe('admitted'));
    stop = firstSubmissions(server.client).subscribe(() => { updates++; });
    const count = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
    server.held.add('session/checkpoint');
    let recovery!: Promise<unknown>;
    await act(async () => { recovery = server.client.refresh(session.id); });
    const checkpoint = await server.waitFor('session/checkpoint', count + 1);
    const proof = server.client.getSnapshot().views[session.id].attachmentObservation;
    expect(server.client.isAttachmentObservationCurrent(session.id, proof)).toBe(true);
    expect(server.client.isAttachmentControlCurrent(session.id, proof)).toBe(false);
    await act(async () => releaseHome());
    await waitFor(() => expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe('session-draft'));
    expect(updates).toBe(0);
    await act(async () => { server.held.delete('session/checkpoint'); server.reply(checkpoint); await recovery; });
    expect(server.client.isAttachmentControlCurrent(session.id, proof)).toBe(true);
    expect(updates).toBe(0);
    await waitFor(() => expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe('session'));
    expect(document.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation')).toBe(incarnation);
    expect(server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'thread/resume' && server.sessionForThread(row.request.params.threadId) === session.id)).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'inbound/queue')).toHaveLength(1);
  } finally { stop(); releaseHome(); }
});

const recoveringReturns = ['Settings', 'Workflows', 'Scheduled Tasks'].flatMap(page => [false, true].map(resync => ({ page, resync })));
it.each(recoveringReturns)('a retained upload failure from $page can recover and publish its Session (resync: $resync)', async ({ page, resync }) => {
  server.held.add('session/uploadStatus');
  const { session, create, incarnation } = await firstSend(undefined, new File(['local attachment'], 'recover.txt'));
  await navigate(page);
  const releaseHome = await holdHomeAuthorization();
  let checkpoint: Request | undefined, recovery: Promise<unknown> | undefined;
  const settleResync = async () => {
    if (!checkpoint) return;
    const request = checkpoint; checkpoint = undefined;
    server.held.delete('session/checkpoint'); server.reply(request); await recovery;
  };
  try {
    await act(async () => server.reply(create));
    const status = await server.waitFor('session/uploadStatus', 1);
    await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: status.id, error: { code: -32000, message: 'Ready read lost', data: { kind: 'operation_failed' } } }));
    await waitFor(() => expect(firstSubmissions(server.client).session(session.id)).toMatchObject({ phase: 'uncertain', failedPhase: 'uploading' }));
    if (resync) {
      const count = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
      server.held.add('session/checkpoint');
      await act(async () => { recovery = server.client.refresh(session.id); });
      checkpoint = await server.waitFor('session/checkpoint', count + 1);
    }
    await act(async () => releaseHome());
    await waitFor(() => expect(primary('Home').getAttribute('aria-current')).toBe('page'));
    await act(settleResync);
    server.held.delete('session/uploadStatus');
    await act(async () => fireEvent.click(await screen.findByRole('button', { name: 'Check status for recover.txt' })));
    await waitFor(() => expect(firstSubmissions(server.client).session(session.id)?.phase).toBe('paused'));
    expect(server.requests.filter(row => row.request.method === 'inbound/queue')).toHaveLength(0);
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Continue submission' })));
    await waitFor(() => expect(firstSubmissions(server.client).session(session.id)?.phase).toBe('admitted'));
    expect(server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'thread/resume' && server.sessionForThread(row.request.params.threadId) === session.id)).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare')).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'inbound/queue')).toHaveLength(1);
    expect(server.carrierTransfers).toBe(1);
    await waitFor(() => expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe('session'));
    expect(document.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation')).toBe(incarnation);
  } finally { releaseHome(); await act(settleResync); }
});

const discardedReturns = ['Settings', 'Workflows', 'Scheduled Tasks'].flatMap(page => [false, true].map(replace => ({ page, replace })));
it.each(discardedReturns)('discarding the first input after $page follows only its retained editor when native resync settles (replace: $replace)', async ({ page, replace }) => {
  server.held.add('session/uploadStatus');
  const { session, create } = await firstSend(undefined, new File(['discarded attachment'], 'discard.txt'));
  await navigate(page);
  const releaseHome = await holdHomeAuthorization();
  let checkpoint: Request | undefined, recovery: Promise<unknown> | undefined;
  const settleResync = async () => {
    if (!checkpoint) return;
    const request = checkpoint; checkpoint = undefined;
    server.held.delete('session/checkpoint'); server.reply(request); await recovery;
  };
  try {
    await act(async () => server.reply(create));
    const status = await server.waitFor('session/uploadStatus', 1);
    await act(async () => server.socket.deliver({ jsonrpc: '2.0', id: status.id, error: { code: -32000, message: 'Ready read lost', data: { kind: 'operation_failed' } } }));
    await waitFor(() => expect(firstSubmissions(server.client).session(session.id)?.phase).toBe('uncertain'));
    const count = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
    server.held.add('session/checkpoint');
    await act(async () => { recovery = server.client.refresh(session.id); });
    checkpoint = await server.waitFor('session/checkpoint', count + 1);
    await act(async () => releaseHome());
    await waitFor(() => expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe('session-draft'));
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Discard first-submission draft' })));
    expect(firstSubmissions(server.client).session(session.id)?.phase).toBe('discarded');
    expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe('session-draft');
    expect((screen.getByRole('textbox', { name: 'Message' }) as HTMLTextAreaElement).value).toBe('');
    if (replace) await act(async () => fireEvent.click(screen.getByRole('button', { name: 'New Conversation' })));
    const incarnation = document.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation');
    await act(settleResync);
    expect(server.client.isAttachmentControlCurrent(session.id, server.client.getSnapshot().views[session.id].attachmentObservation)).toBe(true);
    await waitFor(() => expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe(replace ? 'new-conversation' : 'session'));
    expect(document.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation')).toBe(incarnation);
    expect(server.requests.filter(row => row.request.method === 'inbound/queue')).toHaveLength(0);
    expect(server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'thread/resume' && server.sessionForThread(row.request.params.threadId) === session.id)).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'session/uploadPrepare')).toHaveLength(1);
    if (replace) return;
    fireEvent.change(screen.getByRole('textbox', { name: 'Message' }), { target: { value: 'After discard' } });
    await waitFor(() => expect(screen.getByRole('button', { name: 'Send' })).toHaveProperty('disabled', false));
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
    const sent = await server.waitFor('inbound/queue', 1);
    expect(sent.method === 'inbound/queue' && sent.params.content).toEqual([{ type: 'text', text: 'After discard' }]);
  } finally { releaseHome(); await act(settleResync); }
});

it('primary page return preserves the selected HTML iframe instead of only its tab and cached bytes', async () => {
  const data = new TextEncoder().encode('<input value="Local document state"><button>Count 0</button>');
  server.workspaceHost.workbench = async (_scope, { request }) => request.kind === 'files'
    ? { entries: [{ name: 'interactive.html', directory: false, link: false }] }
    : request.kind === 'resolve' ? { path: request.path }
      : request.kind === 'preview' ? { preview: { data, version: 'v1', bytes: data.length, offset: 0, eof: true } }
        : { terminals: [], shells: [] };
  await mounted();
  await act(async () => fireEvent.click(document.querySelector('[data-preview-toggle]')!));
  await act(async () => fireEvent.click(await screen.findByRole('button', { name: /Workspace files/ })));
  await act(async () => fireEvent.click(await screen.findByRole('button', { name: 'interactive.html' })));
  const frame = await screen.findByTitle('HTML');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Collapse right sidebar' })));
  await act(async () => fireEvent.click(document.querySelector('[data-preview-toggle]')!));
  expect(screen.getByTitle('HTML')).toBe(frame);
  for (const destination of ['Settings', 'Workflows', 'Scheduled Tasks']) {
    await navigate(destination);
    expect(frame.isConnected).toBe(true);
    expect(frame.closest('[hidden]')).toBeTruthy();
    await navigate('Home');
    expect(screen.getByTitle('HTML')).toBe(frame);
  }
});

it('authority replacement cannot activate a model dialog owned by the hidden Settings page', async () => {
  const source = cfg3Source();
  source.user.authored!.models = cfg3Effective().document.models;
  server.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: boundedSettings(source) }));
  await mounted();
  await navigate('Settings');
  await act(async () => fireEvent.click(screen.getByRole('tab', { name: 'Models' })));
  await act(async () => fireEvent.click(await screen.findByRole('row', { name: 'transport' })));
  await act(async () => fireEvent.click(await screen.findByRole('row', { name: 'main' })));
  const dialog = await screen.findByRole('dialog', { name: 'Model main' });
  await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));
  await act(async () => { server.authorityId = 'replacement'; await server.connect(); });
  await waitFor(() => expect(document.querySelector('[data-primary-navigation] button[aria-label="Home"]')!.getAttribute('aria-current')).toBe('page'));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Model main' })).toBeNull());
  expect(document.querySelector('[data-settings-page]')!.closest('[hidden]')).toBeTruthy();
  expect(document.activeElement?.closest('[hidden], [inert]')).toBeNull();
  primary('Home').focus();
  expect(document.activeElement).toBe(primary('Home'));
  await navigate('Settings');
  expect(await screen.findByRole('dialog', { name: 'Model main' })).toBeTruthy();
  expect(server.requests.filter(row => row.request.method === 'configuration/sourceWrite')).toEqual([]);
});

it('hiding a model dialog retains its form while its admitted exact-CAS write settles', async () => {
  const source = cfg3Source();
  source.user.authored!.models = cfg3Effective().document.models;
  server.handlers.set('configuration/sourcesRead', () => ({ type: 'source_settings', projection: boundedSettings(source) }));
  server.handlers.set('configuration/sourceWrite', request => {
    if (request.method !== 'configuration/sourceWrite' || request.params.mutation.kind !== 'config' || request.params.mutation.mutation.unit !== 'model' || !request.params.mutation.mutation.authored) throw Error('Expected authored Model write');
    expect(request.params.expected_revision).toBe('user-1');
    source.user.authored!.models!.main = request.params.mutation.mutation.authored;
    source.user.revision = 'user-2';
    return { type: 'sources_written', revision: 'user-2', configuration_revision: '1' };
  });
  await mounted();
  await navigate('Settings');
  await act(async () => fireEvent.click(screen.getByRole('tab', { name: 'Models' })));
  await act(async () => fireEvent.click(await screen.findByRole('row', { name: 'transport' })));
  await act(async () => fireEvent.click(await screen.findByRole('row', { name: 'main' })));
  const field = screen.getByLabelText('Context window') as HTMLInputElement;
  fireEvent.change(field, { target: { value: '256000' } });
  server.held.add('configuration/sourceWrite');
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save Model main' })));
  const write = await server.waitFor('configuration/sourceWrite', 1);
  const home = document.querySelector<HTMLButtonElement>('[data-primary-navigation] button[aria-label="Home"]')!;
  await act(async () => fireEvent.click(home));
  await waitFor(() => expect(home.getAttribute('aria-current')).toBe('page'));
  expect(field.isConnected).toBe(true);
  expect(field.closest('[hidden]')).toBeTruthy();
  await act(async () => server.reply(write));
  await navigate('Settings');
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save Model main' })).toHaveProperty('disabled', true));
  expect((screen.getByLabelText('Context window') as HTMLInputElement).value).toBe('256000');
  expect(server.requests.filter(row => row.request.method === 'configuration/sourceWrite')).toHaveLength(1);
});

it.each(['Settings', 'Workflows', 'Scheduled Tasks'])('a late created Session deleted during %s Home authorization is not restored', async page => {
  const { session, create, incarnation } = await firstSend();
  await navigate(page);
  const releaseHome = await holdHomeAuthorization();
  try {
    await act(async () => server.reply(create));
    await waitFor(() => expect(firstSubmissions(server.client).session(session.id)?.phase).toBe('admitted'));
    server.handlers.set('session/delete', () => ({ type: 'deletion', result: { status: 'deleted', session_id: session.id } }));
    await act(async () => { await server.client.deleteSession(session.id, 'revision-created'); });
    expect(server.client.getSnapshot().views[session.id]).toBeUndefined();
    await act(async () => releaseHome());
    await waitFor(() => expect(document.querySelector('#session-view')!.getAttribute('data-shell-route')).toBe('new-conversation'));
    expect(document.querySelector('[data-editor-incarnation]')!.getAttribute('data-editor-incarnation')).not.toBe(incarnation);
    expect(screen.getByRole('textbox', { name: 'Message' })).toHaveProperty('value', '');
    expect(server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'thread/resume' && server.sessionForThread(row.request.params.threadId) === session.id)).toHaveLength(1);
    expect(server.requests.filter(row => row.request.method === 'inbound/queue')).toHaveLength(1);
  } finally { releaseHome(); }
});
