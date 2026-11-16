import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { TaskManagerPage } from '../src/app/scheduler/TaskManagerPage';
import { WorkspaceAuthority } from '../src/workspaces/authority';
import { localeController } from '../src/locale/controller';
import { schedulerFixture } from './scheduler-fixture';
import type { Server } from './fixture';
import type { SessionModelConfig } from '../../protocol/app-server/v78';

const servers: Server[] = [];
afterEach(() => { cleanup(); for (const server of servers.splice(0)) server.client.dispose(); localStorage.clear(); localeController.setLocale('en'); vi.restoreAllMocks(); });
async function setup() {
  localeController.setLocale('en');
  const f = schedulerFixture([]); servers.push(f.server); await f.server.connect();
  const openSession = vi.fn();
  const authority = new WorkspaceAuthority(f.server.workspaceHost);
  const ui = render(<TaskManagerPage client={f.server.client} host={f.server.workspaceHost} authority={authority} openSession={openSession}/>);
  await screen.findByRole('button', { name: 'New automation task' });
  return { ...f, openSession, authority, ui };
}
const mutations = (server: Server) => server.requests.filter(row => ['thread/start', 'schedule/command', 'thread/resume', 'turn/start'].includes(row.request.method));

it.each(['untouched', 'profile', 'model'] as const)('preserves effective model policy on the first %s task gesture', async gesture => {
  const f = await setup(), read = f.server.workspaceHost.configureWorkspace!;
  const auxiliary = { mode: 'explicit' as const, model: 'fixture/second', profile: 'medium', request_params: { temperature: 0.1 }, max_output_tokens: 64 };
  const independent = { summaryModel: auxiliary, titleModel: auxiliary, classificationModel: auxiliary, btwModel: auxiliary };
  const configured: SessionModelConfig = { model: 'fixture/native', profile: 'medium', requestParams: { temperature: 0.2 }, maxOutputTokens: 1234, ...independent };
  f.server.workspaceHost.configureWorkspace = async (...args) => {
    const result = await read(...args);
    if (result.kind !== 'read' || result.projection.session_models?.kind !== 'available') throw Error('workspace model source');
    return { ...result, projection: { ...result.projection, session_models: { ...result.projection.session_models, default_model: configured } } };
  };
  fireEvent.click(screen.getByRole('button', { name: 'New' })); await fill();
  if (gesture !== 'untouched') {
    fireEvent.click(screen.getByRole('button', { name: 'Model and thinking effort' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: gesture === 'profile' ? 'Thinking effort' : 'Model' }));
    await act(async () => fireEvent.click(await screen.findByRole('menuitemradio', { name: gesture === 'profile' ? 'high' : 'fixture/second' })));
  }
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  const request = await f.server.waitFor('thread/start', 1);
  expect(request.params).toEqual({ settings: { cwd: '/workspace', ...(gesture === 'untouched' ? {} : { model: gesture === 'profile' ? { ...configured, profile: 'high' } : { model: 'fixture/second', ...independent } }) } });
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(f.server.requests.some(row => ['thread/resume', 'turn/start'].includes(row.request.method))).toBe(false);
});
async function fill(text = 'Review the latest repository changes.') {
  await screen.findByRole('button', { name: 'Workspace' });
  fireEvent.change(screen.getByRole('textbox', { name: 'Task instruction' }), { target: { value: text } });
  await waitFor(() => expect((screen.getByRole('button', { name: 'Create' }) as HTMLButtonElement).disabled).toBe(false));
}
it('opens the same local dialog from both creation actions and cancels without creating or navigating', async () => {
  const f = await setup();
  fireEvent.click(screen.getByRole('button', { name: 'New' }));
  await screen.findByRole('dialog', { name: 'Set up an automation task' });
  await fill(); expect(mutations(f.server)).toHaveLength(0);
  expect(screen.getByRole('button', { name: 'Time' }).textContent).toContain('09:00');
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull()); expect(f.openSession).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'New automation task' }));
  await screen.findByRole('dialog');
  fireEvent.click(screen.getByRole('button', { name: 'Close task setup' }));
  expect(mutations(f.server)).toHaveLength(0);
});
it('creates a dedicated Session with the default model left unauthored, then saves the exact native daily rule', async () => {
  const f = await setup(); fireEvent.click(screen.getByRole('button', { name: 'New' }));
  const text = 'Review changes\nInclude suggested next steps.'; await fill(text);
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  const create = f.server.requests.find(row => row.request.method === 'thread/start')!.request;
  expect(create.params).toEqual({ settings: { cwd: '/workspace' } });
  const schedule = f.server.requests.find(row => row.request.method === 'schedule/command')!.request;
  expect(schedule.params).toMatchObject({ session_id: 'scheduled-1', node_id: 'node-scheduled-1', command: { mutation: { type: 'create', title: 'Review changes', text,
    timing: { type: 'cron', expression: '0 9 * * *', time_zone: Intl.DateTimeFormat().resolvedOptions().timeZone } } } });
  expect(f.server.requests.some(row => ['thread/resume', 'turn/start'].includes(row.request.method))).toBe(false);
  expect(f.openSession).not.toHaveBeenCalled();
  await screen.findByDisplayValue('Review changes');
});
it('uses the registered Workspace and explicit native model/profile selection without changing source configuration', async () => {
  const f = await setup(), configure = vi.fn(f.server.workspaceHost.configureWorkspace!); f.server.workspaceHost.configureWorkspace = configure;
  fireEvent.click(screen.getByRole('button', { name: 'New' })); await fill();
  fireEvent.click(screen.getByRole('button', { name: 'Workspace' })); fireEvent.click(await screen.findByRole('menuitem', { name: 'Workspace B' }));
  await waitFor(() => expect((screen.getByRole('button', { name: 'Create' }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole('button', { name: 'Model and thinking effort' }));
  fireEvent.click(await screen.findByRole('menuitem', { name: 'Model' }));
  await act(async () => fireEvent.click(await screen.findByRole('menuitemradio', { name: 'fixture/second' })));
  fireEvent.click(screen.getByRole('button', { name: 'Model and thinking effort' }));
  fireEvent.click(await screen.findByRole('menuitem', { name: 'Thinking effort' }));
  await act(async () => fireEvent.click(await screen.findByRole('menuitemradio', { name: 'high' })));
  fireEvent.change(screen.getByRole('textbox', { name: 'Task name' }), { target: { value: 'Explicit name' } });
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  const create = await f.server.waitFor('thread/start', 1);
  expect(create.params).toEqual({ settings: { cwd: '/workspace/B', model: { model: 'fixture/second', profile: 'high' } } });
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(configure.mock.calls.every(call => call[2].kind === 'read')).toBe(true);
});
it('retains acknowledged conversation ownership after failure and retries once across close/reopen', async () => {
  const f = await setup(), handler = f.server.handlers.get('schedule/command')!; let failed = false;
  f.server.handlers.set('schedule/command', request => { if (!failed) { failed = true; return { type: 'schedule_command', result: { status: 'invalid', reason: 'fixture failure' } }; } return handler(request); });
  fireEvent.click(screen.getByRole('button', { name: 'New' })); await fill();
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  await screen.findByText(/The task conversation was created/);
  expect((screen.getByRole('textbox', { name: 'Task instruction' }) as HTMLTextAreaElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  fireEvent.click(screen.getByRole('button', { name: 'New' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Retry creation' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(f.server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
  const commands = f.server.requests.filter(row => row.request.method === 'schedule/command').map(row => row.request.params);
  expect(commands).toHaveLength(2); expect(commands[0]).toEqual(commands[1]);
});
it('blocks oversized instructions and disconnected submission before mutation', async () => {
  const f = await setup(); fireEvent.click(screen.getByRole('button', { name: 'New' })); await fill('界'.repeat(5500));
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  await screen.findByText('Keep the task content within 16 KiB.'); expect(mutations(f.server)).toHaveLength(0);
  await act(async () => f.server.socket.close());
  expect((screen.getByRole('button', { name: 'Create' }) as HTMLButtonElement).disabled).toBe(true);
});
it('restores a submitted draft after route remount and retries the original conversation', async () => {
  const f = await setup(), handler = f.server.handlers.get('schedule/command')!; let failed = false;
  f.server.handlers.set('schedule/command', request => { if (!failed) { failed = true; return { type: 'schedule_command', result: { status: 'invalid', reason: 'fixture failure' } }; } return handler(request); });
  fireEvent.click(screen.getByRole('button', { name: 'New' })); await fill('Recover my submitted task');
  fireEvent.click(screen.getByRole('button', { name: 'Create' })); await screen.findByRole('button', { name: 'Retry creation' });
  f.ui.unmount();
  render(<TaskManagerPage client={f.server.client} host={f.server.workspaceHost} authority={f.authority} openSession={f.openSession}/>);
  fireEvent.click(screen.getByRole('button', { name: 'New' }));
  expect((await screen.findByRole('textbox', { name: 'Task instruction' }) as HTMLTextAreaElement).value).toBe('Recover my submitted task');
  fireEvent.click(await screen.findByRole('button', { name: 'Retry creation' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(f.server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
});
it('allows explicit editing after a definitive scheduling rejection without replaying the rejected command', async () => {
  const f = await setup();
  f.server.handlers.set('schedule/command', () => ({ type: 'schedule_command', result: { status: 'invalid', reason: 'rejected timing' } }));
  fireEvent.click(screen.getByRole('button', { name: 'New' })); await fill(); fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Edit task' }));
  await waitFor(() => expect((screen.getByRole('textbox', { name: 'Task instruction' }) as HTMLTextAreaElement).disabled).toBe(false));
  expect(f.server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
  expect(f.server.requests.filter(row => row.request.method === 'schedule/command')).toHaveLength(1);
});
it('requires explicit Start over and Create gestures after a lost conversation creation response', async () => {
  const f = await setup(); f.server.held.add('thread/start');
  fireEvent.click(screen.getByRole('button', { name: 'New' })); await fill('Preserve this task instruction');
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  const request = await f.server.waitFor('thread/start', 1);
  await act(async () => { f.server.commit(request); f.server.socket.close(); });
  await screen.findByRole('button', { name: 'Start over' });
  expect((screen.getByRole('textbox', { name: 'Task instruction' }) as HTMLTextAreaElement).disabled).toBe(true);
  await act(async () => { await f.server.connect(); }); f.server.held.delete('thread/start');
  expect(f.server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
  expect(f.server.requests.filter(row => row.request.method === 'schedule/command')).toHaveLength(0);
  fireEvent.click(screen.getByRole('button', { name: 'Start over' }));
  await waitFor(() => expect((screen.getByRole('textbox', { name: 'Task instruction' }) as HTMLTextAreaElement).disabled).toBe(false));
  expect((screen.getByRole('textbox', { name: 'Task instruction' }) as HTMLTextAreaElement).value).toBe('Preserve this task instruction');
  expect(f.server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(1);
  expect(f.server.requests.filter(row => row.request.method === 'schedule/command')).toHaveLength(0);
  await waitFor(() => expect((screen.getByRole('button', { name: 'Create' }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole('button', { name: 'Create' }));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(f.server.requests.filter(row => row.request.method === 'thread/start')).toHaveLength(2);
  expect(f.server.requests.filter(row => row.request.method === 'schedule/command')).toHaveLength(1);
});
it('shows workspace read failures and a read-only retry inside the modal', async () => {
  const f = await setup(), read = f.server.workspaceHost.configureWorkspace!;
  const configure = vi.fn(read).mockRejectedValueOnce(new Error('Workspace read failed'));
  f.server.workspaceHost.configureWorkspace = configure;
  fireEvent.click(screen.getByRole('button', { name: 'New' }));
  const notice = await screen.findByText('Could not read the workspace settings. Retry before creating the task.');
  const dialog = notice.closest('[role="dialog"]')! as HTMLElement;
  expect(dialog).not.toBeNull();
  expect((within(dialog).getByRole('button', { name: 'Create' }) as HTMLButtonElement).disabled).toBe(true);
  expect(mutations(f.server)).toHaveLength(0);
  fireEvent.click(within(dialog).getByRole('button', { name: 'Retry' }));
  await waitFor(() => expect(screen.queryByText('Could not read the workspace settings. Retry before creating the task.')).toBeNull());
  await fill('Create only after reading the workspace');
  expect(configure).toHaveBeenCalledTimes(2);
  expect(configure.mock.calls.every(call => call[2].kind === 'read')).toBe(true);
  expect(mutations(f.server)).toHaveLength(0);
});
