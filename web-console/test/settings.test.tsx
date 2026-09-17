// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
