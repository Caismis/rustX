// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { Settings } from '../src/app/settings/Settings';
import { OutcomeUncertain, RpcFailure } from '../src/client/app-server';
import { cfg3Client, cfg3Session } from './cfg3-fixture';
afterEach(cleanup);
async function open(subject: ReturnType<typeof cfg3Client>, section: string, scope = 'Workspace') {
  render(<Settings client={subject.client} sessionId={cfg3Session} />);
  await screen.findByText('server-frozen-model');
  fireEvent.click(screen.getByRole('tab', { name: scope }));
  fireEvent.click(screen.getByRole('button', { name: section }));
}
it('separates native Effective, User and Workspace; shows process paths without trust UI', async () => {
  const subject = cfg3Client(); render(<Settings client={subject.client} sessionId={cfg3Session} />);
  await screen.findByText('server-frozen-model');
  expect(screen.getByRole('tab', { name: 'Effective' }).getAttribute('aria-selected')).toBe('true');
  expect(screen.queryByText(/trusted/i)).toBeNull();
  expect(screen.getByText('/bound/rustx.toml')).toBeTruthy();
  expect(screen.getByText('/home/user/rustx/.agents')).toBeTruthy();
  expect(screen.queryByRole('button', { name: /^Save / })).toBeNull();
});
it('saves a whole Workspace Provider with explicit credentials without copying User members', async () => {
  const subject = cfg3Client(); await open(subject, 'Providers & Models');
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'transport' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  expect((screen.getByLabelText('Endpoint') as HTMLInputElement).value).toBe('');
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://workspace.invalid' } });
  fireEvent.change(screen.getByLabelText('Environment variable'), { target: { value: 'WORKSPACE_KEY' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider transport' }));
  await screen.findByText(/Source saved. The loaded runtime/);
  expect(subject.request.mock.calls.find(([operation]) => operation.method === 'configuration/sourceWrite')?.[0]).toEqual({ method: 'configuration/sourceWrite', params: { session_id: cfg3Session, expected_revision: 'workspace-1', mutation: { kind: 'config', scope: 'workspace', mutation: { unit: 'provider', id: 'transport', authored: { base_url: 'https://workspace.invalid', credential: { kind: 'environment', variable: 'WORKSPACE_KEY' } } } } } });
  expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/reload')).toHaveLength(0);
  expect(screen.getByText(/Runtime generation 7 · Pending reload/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
  await screen.findByText('Configuration published: 7 → 8.');
});
it('preserves stale drafts and exact revision until a separate explicit review gesture', async () => {
  const subject = cfg3Client(async (operation, source) => {
    if (operation.method === 'configuration/sourceWrite') { source.workspace.revision = 'external-edit'; throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: operation.params.expected_revision, actual: 'external-edit' } }); }
  });
  await open(subject, 'Tools');
  fireEvent.click(screen.getByLabelText('read')); fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByRole('alert'); await screen.findByRole('button', { name: 'Use reviewed revision' });
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite')).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite')).toHaveLength(2));
  const writes = subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite');
  expect(writes[1][0]).toMatchObject({ params: { expected_revision: 'workspace-1' } });
});
it('repairs uncertain writes by rereading and never replays the mutation', async () => {
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/sourceWrite') throw new OutcomeUncertain(); });
  await open(subject, 'Tools'); fireEvent.click(screen.getByLabelText('read')); fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByRole('alert');
  await waitFor(() => expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourcesRead')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([operation]) => operation.method === 'configuration/sourceWrite')).toHaveLength(1);
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
});
it('shows reload refusal with the authoritative old generation', async () => {
  const subject = cfg3Client(async operation => { if (operation.method === 'configuration/reload') throw new Error('busy: admitted work'); });
  render(<Settings client={subject.client} sessionId={cfg3Session} />); await screen.findByText('server-frozen-model');
  fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
  expect((await screen.findByRole('alert')).textContent).toContain('generation 7 remains authoritative');
});
it('edits an independent named-Agent whole resource with inherited model and Plugins off', async () => {
  const subject = cfg3Client(); await open(subject, 'Agents');
  fireEvent.change(screen.getByLabelText('New Agent identity'), { target: { value: 'researcher' } }); fireEvent.click(screen.getByRole('button', { name: 'Add Agent' }));
  expect(screen.getByText(/invoking Attempt's already-frozen effective model/)).toBeTruthy();
  expect((screen.getByLabelText('todo') as HTMLInputElement).checked).toBe(false);
  fireEvent.change(screen.getByLabelText('Description'), { target: { value: 'Research a topic' } });
  fireEvent.change(screen.getByLabelText('Instructions'), { target: { value: 'Read and report findings.' } });
  fireEvent.click(screen.getByLabelText('read')); fireEvent.click(screen.getByLabelText('todo'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Agent researcher' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([operation]) => operation.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { expected_revision: 'missing', mutation: { kind: 'agent', scope: 'workspace', name: 'researcher', authored: { description: 'Research a topic', tools: { builtin: ['read'] }, plugins: { todo: { enabled: true } } } } } }));
});

it('keeps shadowed User resources visible using native shadowing facts', async () => {
  const subject = cfg3Client();
  subject.source.prospective_resources = { ...subject.effective.resources, definitions: [{ family: 'skill', name: 'review', valid: false, location: { scope: 'workspace', path: '/workspace/.agents/skills/review/SKILL.md', shadowed: '/home/user/rustx/.agents/skills/review/SKILL.md' } }] };
  render(<Settings client={subject.client} sessionId={cfg3Session} />);
  await screen.findByText('server-frozen-model');
  fireEvent.click(screen.getByRole('tab', { name: 'User' }));
  fireEvent.click(screen.getByRole('button', { name: 'Skills' }));
  expect(screen.getByText(/Shadowed by Workspace/).textContent).toContain('/home/user/rustx/.agents/skills/review/SKILL.md');
  expect(screen.queryByText(/Root visible/)).toBeNull();
});

it('retains a draft and its original CAS revision across scope and section navigation', async () => {
  const subject = cfg3Client(); await open(subject, 'Tools');
  fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('tab', { name: 'User' }));
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'General' }));
  subject.source.workspace.revision = 'external';
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourcesRead')).toHaveLength(2));
  fireEvent.click(screen.getByRole('tab', { name: 'Workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { expected_revision: 'workspace-1' } }));
});

it('replaces against the newer revision only after an explicit review gesture', async () => {
  let first = true;
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite' && first) { first = false; source.workspace.revision = 'reviewed'; throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: 'workspace-1', actual: 'reviewed' } }); }
  });
  await open(subject, 'Tools'); fireEvent.click(screen.getByLabelText('read'));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Use reviewed revision' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
  await screen.findByText(/Source saved. The loaded runtime/);
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[1][0]).toMatchObject({ params: { expected_revision: 'reviewed', mutation: { mutation: { authored: ['read'] } } } });
});

it.each(['empty', 'omit'] as const)('preserves native %s Tool selection as a distinct semantic-unit operation', async mode => {
  const subject = cfg3Client(); await open(subject, 'Tools');
  fireEvent.click(screen.getByRole('button', { name: `${mode === 'empty' ? 'Save' : 'Remove'} Native Tools` }));
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { mutation: { mutation: { unit: 'native_tools', authored: mode === 'empty' ? [] : null } } } }));
});

it('repairs an uncertain Reload by rereading the native generation without replay', async () => {
  const subject = cfg3Client(async (op, source, effective) => { if (op.method === 'configuration/reload') { effective.generation = '8'; source.loaded = { generation: '8', pending_reload: false, changed_sources: [] }; throw new OutcomeUncertain(); } });
  render(<Settings client={subject.client} sessionId={cfg3Session} />); await screen.findByText('server-frozen-model');
  fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
  await screen.findByText(/Runtime generation 8/);
  expect(screen.getByRole('alert').textContent).toContain('will not be replayed');
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/reload')).toHaveLength(1);
});

it('presents User-only process policy as restart-required and Effective as read-only', async () => {
  const subject = cfg3Client(); await open(subject, 'General', 'User');
  expect(screen.getByText(/take effect after restart/)).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Save App Server policy' })).toBeTruthy();
  fireEvent.click(screen.getByRole('tab', { name: 'Workspace' }));
  expect(screen.queryByRole('button', { name: 'Save App Server policy' })).toBeNull();
  expect(screen.getByText(/User-only and restart-required/)).toBeTruthy();
  fireEvent.click(screen.getByRole('tab', { name: 'Effective' }));
  expect(screen.queryByRole('button', { name: /^Save / })).toBeNull();
  expect(screen.getByText(/not evidence of a live process change/)).toBeTruthy();
});

it('does not equate invalid resource existence with readiness or Root authority', async () => {
  const subject = cfg3Client();
  subject.effective.resources.definitions = [{ name: 'unselected', family: 'managed_python', valid: false, location: { scope: 'workspace', path: '/workspace/.agents/python/unselected' } }];
  subject.effective.resources.sources = { 'python:unselected': { status: 'unavailable' } };
  render(<Settings client={subject.client} sessionId={cfg3Session} />); await screen.findByText('server-frozen-model');
  fireEvent.click(screen.getByRole('button', { name: 'Managed Python' }));
  expect(screen.getByText('Invalid definition')).toBeTruthy(); expect(screen.getByText('Defined only')).toBeTruthy();
  expect(screen.getAllByText('unavailable').length).toBeGreaterThan(0);
  expect(screen.queryByText('Root selected')).toBeNull();
});

it('keeps contributor default intent unspecified when enabling the closed Agent Status Plugin', async () => {
  const subject = cfg3Client(); await open(subject, 'Plugins');
  expect((screen.getByLabelText('Time contributor') as HTMLSelectElement).value).toBe('');
  fireEvent.click(screen.getByRole('switch', { name: 'Enable Agent Status Plugin' }));
  fireEvent.click(screen.getByRole('button', { name: 'Save Agent Status Plugin' }));
  await waitFor(() => expect(subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')?.[0]).toMatchObject({ params: { mutation: { mutation: { unit: 'agent_status', authored: { enabled: true } } } } }));
  const write = subject.request.mock.calls.find(([op]) => op.method === 'configuration/sourceWrite')![0];
  expect(JSON.stringify(write)).not.toMatch(/"time"|"background"|npm|cordis/);
});

it('reconnect rereads native sources without replaying a dirty draft', async () => {
  const subject = cfg3Client(); const ui = render(<Settings client={subject.client} sessionId={cfg3Session} />);
  await screen.findByText('server-frozen-model'); fireEvent.click(screen.getByRole('tab', { name: 'Workspace' })); fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  fireEvent.click(screen.getByLabelText('read'));
  const snapshot = { ...subject.state, generation: 2 };
  subject.client.getSnapshot = () => snapshot;
  ui.rerender(<Settings client={subject.client} sessionId={cfg3Session} />);
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourcesRead')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(0);
  expect((screen.getByLabelText('read') as HTMLInputElement).checked).toBe(true);
});

it('an older authoritative read cannot replace a newer read', async () => {
  let release: (value: import('../../protocol/app-server/v6').MethodResult) => void = () => {};
  let count = 0;
  const subject = cfg3Client(async op => { if (op.method === 'configuration/sourcesRead' && ++count === 2) return new Promise(resolve => { release = resolve; }); });
  render(<Settings client={subject.client} sessionId={cfg3Session} />); await screen.findByText('server-frozen-model');
  const stale = structuredClone(subject.source);
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await waitFor(() => expect(count).toBe(2));
  subject.source.workspace.revision = 'newest';
  fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  fireEvent.click(screen.getByRole('tab', { name: 'Workspace' }));
  await screen.findByText(/Revision: newest/);
  release({ type: 'source_settings', projection: stale, session_revision: '1', session_selection: null });
  await waitFor(() => expect(screen.getByText(/Revision: newest/)).toBeTruthy());
});

it('removes literal credentials from a successful Provider draft using the redacted native acknowledgement', async () => {
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite' && op.params.mutation.kind === 'config' && op.params.mutation.mutation.unit === 'provider') {
      source.workspace.authored = { providers: { secret: { base_url: 'https://native.invalid', credential: { type: 'literal' } } } };
    }
  });
  await open(subject, 'Providers & Models');
  fireEvent.change(screen.getByLabelText('New Provider identity'), { target: { value: 'secret' } }); fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
  fireEvent.change(screen.getByLabelText('Endpoint'), { target: { value: 'https://native.invalid' } }); fireEvent.change(screen.getByLabelText('Credential source'), { target: { value: 'literal' } });
  fireEvent.change(screen.getByLabelText('New literal credential'), { target: { value: 'SECRET_SENTINEL' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save Provider secret' }));
  await screen.findByText(/Source saved. The loaded runtime/);
  await waitFor(() => expect(screen.queryByLabelText('New literal credential')).toBeNull());
  expect(document.body.innerHTML).not.toContain('SECRET_SENTINEL');
  fireEvent.click(screen.getByRole('button', { name: 'Back to catalog' })); fireEvent.click(screen.getByRole('button', { name: 'Edit Provider secret' }));
  expect((screen.getByLabelText('Credential source') as HTMLSelectElement).value).toBe('retain');
});

it('retains a Model draft when native validation rejects its semantic unit', async () => {
 const subject = cfg3Client(async op => { if (op.method === 'configuration/sourceWrite') throw new RpcFailure({ code: -32000, message: 'Native validation failed', data: { kind: 'configuration_failed', diagnostic: 'Invalid provider reference' } }); });
 await open(subject, 'Providers & Models'); fireEvent.change(screen.getByLabelText('New Model identity'), { target: { value: 'draft-model' } }); fireEvent.click(screen.getByRole('button', { name: 'Add Model' }));
 fireEvent.change(screen.getByLabelText('Wire model identity'), { target: { value: 'wire' } }); fireEvent.change(screen.getByLabelText('Provider identity'), { target: { value: 'missing' } });
 fireEvent.click(screen.getByRole('button', { name: 'Save Model draft-model' }));
 expect((await screen.findByRole('alert')).textContent).toContain('Invalid provider reference'); expect((screen.getByLabelText('Provider identity') as HTMLInputElement).value).toBe('missing');
 expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/reload')).toHaveLength(0);
});
it('preserves the original revision when removing an otherwise clean unit conflicts', async () => {
  const subject = cfg3Client(async (op, source) => {
    if (op.method === 'configuration/sourceWrite') {
      source.workspace.revision = 'external-removal-conflict';
      throw new RpcFailure({ code: -32000, message: 'Conflict', data: { kind: 'source_conflict', scope: 'workspace', expected: op.params.expected_revision, actual: source.workspace.revision } });
    }
  });
  await open(subject, 'Tools');
  fireEvent.click(screen.getByRole('button', { name: 'Remove Native Tools' }));
  await screen.findByRole('button', { name: 'Use reviewed revision' });
  fireEvent.click(screen.getByRole('button', { name: 'Remove Native Tools' }));
  await waitFor(() => expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')).toHaveLength(2));
  expect(subject.request.mock.calls.filter(([op]) => op.method === 'configuration/sourceWrite')[1][0]).toMatchObject({ params: { expected_revision: 'workspace-1', mutation: { mutation: { authored: null } } } });
});
it.each([
  ['effect', 'refresh'], ['effect', 'write'], ['refresh', 'refresh'], ['refresh', 'write'],
] as const)('fences an obsolete %s read rejection after a newer %s', async (readKind, successor) => {
  let rejectRead!: (error: Error) => void;
  const pending = new Promise<import('../../protocol/app-server/v6').MethodResult>((_, reject) => { rejectRead = reject; });
  let reads = 0;
  const subject = cfg3Client(async op => {
    if (op.method === 'configuration/sourcesRead' && ++reads === 2) return pending;
  });
  const ui = render(<Settings client={subject.client} sessionId={cfg3Session} />);
  await screen.findByText('server-frozen-model');
  fireEvent.click(screen.getByRole('tab', { name: 'Workspace' }));
  fireEvent.click(screen.getByRole('button', { name: 'Tools' }));
  if (readKind === 'effect') {
    const snapshot = { ...subject.state, generation: 2 };
    subject.client.getSnapshot = () => snapshot;
    ui.rerender(<Settings client={subject.client} sessionId={cfg3Session} />);
  } else fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  await waitFor(() => expect(reads).toBe(2));
  if (successor === 'refresh') {
    subject.source.workspace.revision = 'new-authority';
    fireEvent.click(screen.getByRole('button', { name: 'Read current sources' }));
  } else {
    fireEvent.click(screen.getByLabelText('read'));
    fireEvent.click(screen.getByRole('button', { name: 'Save Native Tools' }));
    await screen.findByText(/Source saved. The loaded runtime/);
  }
  const revision = successor === 'refresh' ? 'new-authority' : 'saved-2';
  await screen.findByText(new RegExp(`Revision: ${revision}`));
  await act(async () => { rejectRead(new Error('obsolete read failed')); await pending.catch(() => {}); });
  expect(screen.getByText(new RegExp(`Revision: ${revision}`))).toBeTruthy();
  expect(screen.queryByRole('alert')).toBeNull();
  expect(document.body.textContent).not.toContain('obsolete read failed');
});
