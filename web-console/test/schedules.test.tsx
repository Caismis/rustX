import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { TaskDetail, TaskDeleteDialog } from '../src/app/scheduler/TaskDetail';
import { App } from '../src/app/App';
import { SessionTasks, TaskNavigationProvider } from '../src/app/scheduler/SessionTasks';
import { schedulerModel, taskKey, taskRef } from '../src/app/scheduler/model';
import { CreatedTasks, createdReceipt } from '../src/app/scheduler/CreatedTasks';
import { JournalSourceProvider } from '../src/app/agent/ObservationSource';
import { checkpoint, snapshot } from './fixture';
import type { ForegroundToolExecution } from '../../protocol/app-server/v61';
import { timingDraft, timingValue, TimingInputError } from '../src/app/scheduler/timing';
import { localeController } from '../src/locale/controller';
import { schedulerFixture, task, header, nodeA, nodeB, workflowFixture } from './scheduler-fixture';
import type { Server } from './fixture';
const servers: Server[] = [];
afterEach(() => { cleanup(); for (const server of servers.splice(0)) server.client.dispose(); localStorage.clear(); vi.restoreAllMocks(); });
async function fixture(values = [task()]) { const f = schedulerFixture(values); servers.push(f.server); await f.server.connect(); return f; }
function detail(server: Server, value = task()) { return render(<TaskDetail client={server.client} task={taskRef(header(value))}/>); }
it('reads cold headers and searches instructions without loading bodies, attaching, or scanning Session trees', async () => {
  const f = await fixture(), model = schedulerModel(f.server.client), stop = model.watchAll();
  await waitFor(() => expect(model.getSnapshot().records).toHaveLength(1));
  expect(await model.searchContent('anything that needs attention', () => true)).toEqual(new Set([taskKey(taskRef(task()))])); stop();
  const reads = f.server.requests.map(row => row.request.method);
  expect(reads).toContain('schedule/catalog'); expect(reads).not.toContain('schedule/get'); expect(reads).not.toContain('session/tree');
  expect(f.server.claims()).toEqual([]); expect(f.server.coldLoads.size).toBe(0);
});
it('preserves known rows when a source is unavailable and converges on a native change hint', async () => {
  const f = await fixture(), model = schedulerModel(f.server.client), stop = model.watchAll();
  await waitFor(() => expect(model.getSnapshot().records).toHaveLength(1));
  f.server.handlers.set('schedule/catalog', () => ({ type: 'schedule_catalog', page: { entries: [], next: null, unavailable: [task().binding] } }));
  await model.refresh(); expect(model.getSnapshot().records).toHaveLength(1); expect(model.getSnapshot().issue).toBe(true);
  f.server.handlers.set('schedule/catalog', () => ({ type: 'schedule_catalog', page: { entries: [], next: null, unavailable: [] } }));
  f.notify(); await waitFor(() => expect(model.getSnapshot().records).toHaveLength(0)); expect(model.getSnapshot().issue).toBe(false); stop();
});
it('hides an empty header clock and opens a single enabled task in its original node', async () => {
  const f = await fixture([]), openTask = vi.fn();
  render(<TaskNavigationProvider value={{ openTask, openTasks: vi.fn() }}><SessionTasks client={f.server.client} sessionId="A" nodeId={nodeA}/></TaskNavigationProvider>);
  await waitFor(() => expect(f.server.requests.some(row => row.request.method === 'schedule/list')).toBe(true));
  expect(screen.queryByRole('button', { name: /automation task/i })).toBeNull();
  f.rows.set(task().id.ordinal, task()); await act(async () => f.notify());
  fireEvent.click(await screen.findByRole('button', { name: '1 automation task' }));
  expect(openTask).toHaveBeenCalledWith(taskRef(task()), task().title); expect(f.server.claims()).toEqual([]);
});
it('edits with exact decimal CAS without resetting interval timing on a title-only change', async () => {
  const value = task(); value.spec = { type: 'interval', every_seconds: 3600, anchor_utc: '2026-10-01T10:00:00Z' };
  const f = await fixture([value]); detail(f.server, value);
  fireEvent.change(await screen.findByRole('textbox', { name: 'Automation task name' }), { target: { value: 'Readable task title' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  const request = await f.server.waitFor('schedule/command', 1);
  if (request.method !== 'schedule/command') throw Error('command');
  expect(request.params).toMatchObject({ session_id: 'A', node_id: nodeA, command: { mutation: { type: 'update', expected_revision: '9007199254740993', id: value.id, title: 'Readable task title' } } });
  expect(request.params.command.mutation).not.toHaveProperty('timing');
  await waitFor(() => expect(screen.queryByText('Unsaved changes')).toBeNull()); expect(f.server.claims()).toEqual([]);
});
it('preserves dirty text on refresh and requires an explicit choice before rebasing', async () => {
  const f = await fixture(); detail(f.server);
  const input = await screen.findByRole('textbox', { name: 'Automation task name' }); fireEvent.change(input, { target: { value: 'My pending edit' } });
  const newer = { ...task(), revision: '9007199254740994', title: 'Changed elsewhere' }; f.rows.set(newer.id.ordinal, newer);
  await act(async () => f.notify()); await screen.findByText('Changed elsewhere');
  expect((input as HTMLInputElement).value).toBe('My pending edit');
  expect((screen.getByRole('button', { name: 'Save changes' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Keep my edits' })); fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  const request = await f.server.waitFor('schedule/command', 1);
  if (request.method !== 'schedule/command' || !('expected_revision' in request.params.command.mutation)) throw Error('CAS');
  expect(request.params.command.mutation.expected_revision).toBe(newer.revision);
});
it('converges on an authoritative version identical to the draft before accepting a different edit', async () => {
  const f = await fixture(); detail(f.server);
  const input = await screen.findByRole('textbox', { name: 'Automation task name' });
  fireEvent.change(input, { target: { value: 'Confirmed intent' } });
  const newer = { ...task(), title: 'Confirmed intent', revision: String(BigInt(task().revision) + 1n) }; f.rows.set(newer.id.ordinal, newer);
  await act(async () => f.notify()); await waitFor(() => expect(screen.queryByText('Unsaved changes')).toBeNull());
  fireEvent.change(input, { target: { value: 'Next intentional edit' } }); fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  const command = await f.server.waitFor('schedule/command', 1);
  expect(command.params).toMatchObject({ command: { mutation: { expected_revision: newer.revision, title: 'Next intentional edit' } } });
});
it('keeps a dirty draft disabled during disconnect and verifies it after reconnect', async () => {
  const f = await fixture(); detail(f.server);
  const input = await screen.findByRole('textbox', { name: 'Automation task name' }); fireEvent.change(input, { target: { value: 'Draft survives reconnect' } });
  await act(async () => f.server.socket.close()); expect((input as HTMLInputElement).value).toBe('Draft survives reconnect');
  expect((screen.getByRole('button', { name: 'Save changes' }) as HTMLButtonElement).disabled).toBe(true);
  await act(async () => f.server.connect());
  await waitFor(() => expect((screen.getByRole('button', { name: 'Save changes' }) as HTMLButtonElement).disabled).toBe(false));
  expect((screen.getByRole('textbox', { name: 'Automation task name' }) as HTMLInputElement).value).toBe('Draft survives reconnect');
  expect(f.server.requests.some(row => row.request.method === 'schedule/command')).toBe(false);
});
it.each([false, true])('preserves unsaved fields through App Workbench reconnect (changed elsewhere: %s)', async changedElsewhere => {
  const value = task(); value.spec = { type: 'interval', every_seconds: 3600, anchor_utc: '2026-10-01T10:00:00Z' };
  const f = await fixture([value]), server = f.server;
  server.summaries.set('A', { active_node: nodeA }); server.nodeSnapshots.set(nodeA, server.snapshots.get('A')!);
  server.handlers.set('workflow/list', request => {
    if (request.method !== 'workflow/list') throw Error('Workflow library');
    return { type: 'workflow_library', library: { target: request.params.target, directory: '/workflows', absent_revision: 'empty', files: [], diagnostic: null } };
  });
  await server.client.attach('A');
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: server.client.getSnapshot().endpoint, openViews: ['A'] }));
  render(<App client={server.client} workspaceHost={server.workspaceHost}/>);
  fireEvent.click(await screen.findByRole('button', { name: '1 automation task' }));
  const original = await screen.findByRole('textbox', { name: 'Automation task name' });
  expect(original.closest('[data-workbench]')).not.toBeNull();
  fireEvent.change(original, { target: { value: 'Keep my Workbench draft' } });
  fireEvent.change(screen.getByLabelText('Task instruction'), { target: { value: 'Unsaved instruction' } });
  fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '2' } });
  const generation = server.client.getSnapshot().generation;
  await act(async () => server.socket.close());
  const revision = changedElsewhere ? String(BigInt(value.revision) + 1n) : value.revision;
  if (changedElsewhere) f.rows.set(value.id.ordinal, { ...value, revision, title: 'Changed while offline' });
  await act(async () => server.connect());
  expect(server.client.getSnapshot().generation).toBeGreaterThan(generation);
  expect(original.isConnected).toBe(false);
  const restored = await screen.findByRole('textbox', { name: 'Automation task name' });
  await waitFor(() => expect((restored as HTMLInputElement).disabled).toBe(false));
  expect((restored as HTMLInputElement).value).toBe('Keep my Workbench draft');
  expect((screen.getByLabelText('Task instruction') as HTMLTextAreaElement).value).toBe('Unsaved instruction');
  expect((screen.getByRole('spinbutton') as HTMLInputElement).value).toBe('2');
  expect(server.requests.filter(row => row.request.method === 'schedule/command')).toHaveLength(0);
  if (changedElsewhere) {
    await screen.findByText('Changed while offline', { selector: 'p' });
    expect((screen.getByRole('button', { name: 'Save changes' }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.click(screen.getByRole('button', { name: 'Keep my edits' }));
  }
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  const command = await server.waitFor('schedule/command', 1);
  expect(command.params).toMatchObject({ command: { mutation: { expected_revision: revision, title: 'Keep my Workbench draft', text: 'Unsaved instruction', timing: { type: 'interval', every_seconds: 7200 } } } });
  await waitFor(() => expect(screen.queryByText('Unsaved changes')).toBeNull());
});
it.each(['a'.repeat(16376), 'é'.repeat(8188)])('allows a name-only Workflow edit at the 16 KiB UTF-8 boundary (%#)', async argument => {
  const value = workflowFixture().task, args = { x: argument };
  value.target = { type: 'workflow', pinned: {}, arguments: args };
  expect(new TextEncoder().encode(JSON.stringify(args)).byteLength).toBe(16384);
  expect(new TextEncoder().encode(JSON.stringify(args, null, 2)).byteLength).toBe(16389);
  const f = await fixture([value]); detail(f.server, value);
  fireEvent.change(await screen.findByRole('textbox', { name: 'Automation task name' }), { target: { value: 'Boundary-sized Workflow' } });
  const save = screen.getByRole('button', { name: 'Save changes' }) as HTMLButtonElement;
  expect(save.disabled).toBe(false); fireEvent.click(save);
  const command = await f.server.waitFor('schedule/command', 1);
  if (command.method !== 'schedule/command') throw Error('command');
  expect(command.params.command.mutation).toMatchObject({ type: 'update_metadata', title: 'Boundary-sized Workflow' });
  expect(f.rows.get(value.id.ordinal)?.target).toEqual(value.target);
  expect(command.params.command.mutation).not.toHaveProperty('timing');
  await waitFor(() => expect(screen.queryByText('Unsaved changes')).toBeNull());
});
it('rejects oversized serialized Workflow inputs and malformed JSON while ignoring formatting whitespace', async () => {
  const value = workflowFixture().task, f = await fixture([value]); detail(f.server, value);
  const input = await screen.findByLabelText('Workflow inputs');
  fireEvent.change(input, { target: { value: JSON.stringify({ x: 'é'.repeat(8189) }) } });
  expect((screen.getByRole('button', { name: 'Save changes' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText('Keep the task content within 16 KiB.')).toBeTruthy();
  fireEvent.change(input, { target: { value: '{ invalid' } });
  expect((screen.getByRole('button', { name: 'Save changes' }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText('Enter valid JSON for the workflow inputs.')).toBeTruthy();
  fireEvent.change(input, { target: { value: ' '.repeat(16384) + '{"branch":"main"}' } });
  expect((screen.getByRole('button', { name: 'Save changes' }) as HTMLButtonElement).disabled).toBe(false);
  expect(f.server.requests.filter(row => row.request.method === 'schedule/command')).toHaveLength(0);
});
it.each(['name', 'timing'] as const)('edits Workflow %s without replacing its saved target', async field => {
  const value = workflowFixture().task; value.spec = { type: 'interval', every_seconds: 3600, anchor_utc: '2026-10-01T10:00:00Z' };
  const f = await fixture([value]); detail(f.server, value);
  const name = await screen.findByRole('textbox', { name: 'Automation task name' });
  if (field === 'name') fireEvent.change(name, { target: { value: 'Keep the saved Workflow version' } });
  else fireEvent.change(screen.getByRole('spinbutton'), { target: { value: '2' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  const command = await f.server.waitFor('schedule/command', 1);
  if (command.method !== 'schedule/command') throw Error('command');
  expect(command.params.command.mutation).toMatchObject({ type: 'update_metadata', expected_revision: value.revision });
  expect(command.params.command.mutation).not.toHaveProperty('arguments');
  expect(command.params.command.mutation).not.toHaveProperty('workflow_id');
});
it('does not carry unsaved task contents to a replacement App Server authority', async () => {
  const f = await fixture(), ui = detail(f.server);
  fireEvent.change(await screen.findByRole('textbox', { name: 'Automation task name' }), { target: { value: 'Private draft for the original authority' } });
  ui.unmount();
  await act(async () => { f.server.socket.close(); f.server.authorityId = 'replacement-server'; await f.server.connect(); });
  detail(f.server);
  await screen.findByDisplayValue(task().title);
  expect(screen.queryByDisplayValue('Private draft for the original authority')).toBeNull();
  expect(screen.queryByText('Unsaved changes')).toBeNull();
});
it.each([{ workflow: false, newer: false }, { workflow: true, newer: false }, { workflow: true, newer: true }])('reconciles normalized recovered intent while preserving newer local edits (workflow: $workflow; newer: $newer)', async ({ workflow, newer }) => {
  const value = workflow ? workflowFixture().task : task(), f = await fixture([value]); detail(f.server, value);
  const name = await screen.findByRole('textbox', { name: 'Automation task name' });
  fireEvent.change(name, { target: { value: ' Recovered intent ' } });
  if (workflow) fireEvent.change(screen.getByLabelText('Workflow inputs'), { target: { value: '{"branch":"release"}' } });
  f.server.held.add('schedule/command'); fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  const first = await f.server.waitFor('schedule/command', 1);
  await act(async () => { f.server.socket.close(); await f.server.connect(); });
  const model = schedulerModel(f.server.client), editor = model.editor(value);
  await waitFor(() => expect(editor.getSnapshot().error).toBe('uncertain'));
  if (newer) act(() => editor.edit({ ...editor.getSnapshot().draft, title: 'Newer local intent' }));
  const recover = await screen.findByRole('button', { name: 'Retry previous change' });
  await waitFor(() => expect((recover as HTMLButtonElement).disabled).toBe(false));
  f.server.held.delete('schedule/command'); fireEvent.click(recover);
  await waitFor(() => expect(editor.getSnapshot().baseline.revision).toBe(String(BigInt(value.revision) + 1n)));
  expect(editor.getSnapshot().incoming).toBeUndefined(); expect(editor.getSnapshot().error).toBeUndefined();
  expect(editor.getSnapshot().draft.title).toBe(newer ? 'Newer local intent' : 'Recovered intent');
  expect(editor.dirty).toBe(newer);
  if (workflow) expect(editor.getSnapshot().baseline.values.content).toBe(JSON.stringify({ branch: 'release' }, null, 2));
  const second = f.server.requests.filter(row => row.request.method === 'schedule/command')[1]?.request;
  if (first.method !== 'schedule/command' || second?.method !== 'schedule/command') throw Error('command');
  expect(second.params.command.command_id).toBe(first.params.command.command_id);
  expect(model.getSnapshot().uncertain.size).toBe(0);
});

it('records an acknowledged edit after its task view has unmounted', async () => {
  const f = await fixture(), ui = detail(f.server); f.server.held.add('schedule/command');
  fireEvent.change(await screen.findByRole('textbox', { name: 'Automation task name' }), { target: { value: 'Saved after closing the view' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  const command = await f.server.waitFor('schedule/command', 1); ui.unmount();
  await act(async () => f.server.reply(command));
  await waitFor(() => expect(schedulerModel(f.server.client).getSnapshot().notice).toBe('updated'));
  detail(f.server); await screen.findByDisplayValue('Saved after closing the view');
  await waitFor(() => expect(screen.queryByText('Unsaved changes')).toBeNull());
  expect(f.server.requests.filter(row => row.request.method === 'schedule/command')).toHaveLength(1);
});
it('retains an uncertain edit through remount and retries only on explicit recovery with the original command ID', async () => {
  const f = await fixture(), ui = detail(f.server); f.server.held.add('schedule/command');
  fireEvent.change(await screen.findByRole('textbox', { name: 'Automation task name' }), { target: { value: 'Retry this exact intent' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  const first = await f.server.waitFor('schedule/command', 1); ui.unmount();
  await act(async () => { f.server.socket.close(); await f.server.connect(); });
  detail(f.server); await screen.findByDisplayValue('Retry this exact intent');
  const save = screen.getByRole('button', { name: 'Retry previous change' }) as HTMLButtonElement;
  await waitFor(() => expect(save.disabled).toBe(false));
  expect(f.server.requests.filter(row => row.request.method === 'schedule/command')).toHaveLength(1);
  fireEvent.click(save); const retry = await f.server.waitFor('schedule/command', 2);
  expect(retry.params).toEqual(first.params);
  await act(async () => f.server.reply(retry));
  await waitFor(() => expect(screen.queryByText('Unsaved changes')).toBeNull());
});
it('can recover an uncertain original command after Cancel and remount, then admit a different edit', async () => {
  const f = await fixture(), ui = detail(f.server); f.server.held.add('schedule/command');
  fireEvent.change(await screen.findByRole('textbox', { name: 'Automation task name' }), { target: { value: 'Original uncertain intent' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  const first = await f.server.waitFor('schedule/command', 1);
  await act(async () => { f.server.socket.close(); await f.server.connect(); });
  await waitFor(() => expect((screen.getByRole('button', { name: 'Cancel' }) as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' })); ui.unmount();
  detail(f.server); await screen.findByDisplayValue(task().title);
  expect(f.rows.get(task().id.ordinal)?.revision).toBe(task().revision);
  expect(f.server.requests.filter(row => row.request.method === 'schedule/command')).toHaveLength(1);
  const recover = await screen.findByRole('button', { name: 'Retry previous change' });
  fireEvent.click(recover); const retry = await f.server.waitFor('schedule/command', 2);
  expect(retry.params).toEqual(first.params);
  await act(async () => f.server.reply(retry));
  const input = await screen.findByRole('textbox', { name: 'Automation task name' });
  await waitFor(() => expect((input as HTMLInputElement).disabled).toBe(false));
  expect((input as HTMLInputElement).value).toBe('Original uncertain intent');
  fireEvent.change(input, { target: { value: 'A different edit after recovery' } });
  f.server.held.delete('schedule/command'); fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
  const next = await f.server.waitFor('schedule/command', 3);
  if (first.method !== 'schedule/command' || next.method !== 'schedule/command') throw Error('command');
  expect(next.params.command.command_id).not.toBe(first.params.command.command_id);
  expect(next.params.command.mutation).toMatchObject({ title: 'A different edit after recovery', expected_revision: String(BigInt(task().revision) + 1n) });
  await waitFor(() => expect(screen.queryByText('Unsaved changes')).toBeNull());
});
it('rejects a late body from a different selected node', async () => {
  const second = task('Second task', 'B', nodeB, '2'), f = await fixture([task(), second]); f.server.held.add('schedule/get');
  const ui = detail(f.server); const old = await f.server.waitFor('schedule/get', 1);
  ui.rerender(<TaskDetail client={f.server.client} task={taskRef(second)}/>);
  const fresh = await f.server.waitFor('schedule/get', 2); await act(async () => f.server.reply(fresh));
  await screen.findByDisplayValue('Second task'); await act(async () => f.server.reply(old)); expect(screen.queryByDisplayValue(task().title)).toBeNull();
});
it('confirms deletion and closes only after native acknowledgement', async () => {
  const f = await fixture(), closed = vi.fn(); f.server.held.add('schedule/command');
  render(<TaskDeleteDialog client={f.server.client} record={header(task())} close={closed}/>);
  expect(f.server.requests.some(row => row.request.method === 'schedule/command')).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'Delete task' })); const request = await f.server.waitFor('schedule/command', 1);
  expect(closed).not.toHaveBeenCalled(); await act(async () => f.server.reply(request)); await waitFor(() => expect(closed).toHaveBeenCalledOnce());
});
it('reuses the command ID for an explicit uncertain retry, never replaying on reconnect', async () => {
  const f = await fixture(), model = schedulerModel(f.server.client), mutation = { type: 'set_enabled' as const, id: task().id, expected_revision: task().revision, enabled: false };
  f.server.held.add('schedule/command'); const first = model.change(taskRef(task()), mutation).catch(error => error);
  const original = await f.server.waitFor('schedule/command', 1); f.server.socket.close();
  expect((await first).kind).toBe('uncertain'); await f.server.connect();
  expect(f.server.requests.filter(row => row.request.method === 'schedule/command')).toHaveLength(1);
  const retried = model.change(taskRef(task()), mutation); const again = await f.server.waitFor('schedule/command', 2);
  expect(again.params).toEqual(original.params); f.server.reply(again); await retried;
});
it('retains an admitted deletion through view disposal and records only its acknowledged outcome', async () => {
  const f = await fixture(), model = schedulerModel(f.server.client); f.server.held.add('schedule/command');
  const ui = render(<TaskDeleteDialog client={f.server.client} record={header(task())} close={vi.fn()}/>);
  fireEvent.click(screen.getByRole('button', { name: 'Delete task' }));
  const command = await f.server.waitFor('schedule/command', 1); ui.unmount();
  expect(model.getSnapshot().notice).toBeUndefined();
  await act(async () => f.server.reply(command));
  await waitFor(() => expect(model.getSnapshot().notice).toBe('deleted'));
  expect(f.rows.size).toBe(0); expect(f.server.claims()).toEqual([]);
});
it('shows readable Workflow state with bounded steps and separate technical details', async () => {
  const value = workflowFixture(), f = await fixture([value.task]);
  f.server.handlers.set('workflow/scheduled/getRecovery', () => ({ type: 'scheduled_workflow_recovery', recovery: value.recovery }));
  detail(f.server, value.task); await screen.findByRole('textbox', { name: 'Automation task name' }); fireEvent.click(screen.getByRole('tab', { name: 'Task runs' }));
  await screen.findByText('In progress'); expect(screen.getByText(/still uses a task slot/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Load more steps' })); const next = await f.server.waitFor('workflow/scheduled/getRecovery', 2);
  if (next.method !== 'workflow/scheduled/getRecovery') throw Error('recovery');
  expect(next.params.after_effect_ordinal).toBe('9007199254740993'); expect(f.server.claims()).toEqual([]);
});
it('provides Chinese labels and keyboard tab navigation', async () => {
  const f = await fixture(); localeController.setLocale('zh'); detail(f.server); await screen.findByRole('textbox', { name: '自动化任务名称' });
  fireEvent.keyDown(screen.getByRole('tab', { name: '规则' }), { key: 'ArrowRight' });
  expect(screen.getByRole('tab', { name: '任务运行记录' }).getAttribute('aria-selected')).toBe('true'); expect(screen.getByText('暂无任务运行记录')).toBeTruthy();
});
it('converts local time, rejects DST gaps and cron seconds, and preserves large creation identities', () => {
  const draft = timingDraft({ type: 'at', at_utc: '2028-03-01T10:00:00Z' });
  expect(timingValue({ ...draft, date: '2028-11-01', time: '09:30:00', zone: 'Asia/Shanghai' }, 0)).toEqual({ type: 'at', at: '2028-11-01T01:30:00.000Z' });
  expect(() => timingValue({ ...draft, date: '2028-03-12', time: '02:30:00', zone: 'America/New_York' }, 0)).toThrow(TimingInputError);
  expect(() => timingValue({ ...draft, kind: 'daily', time: '09:30:01' }, 0)).toThrow(TimingInputError);
  expect(createdReceipt('{"status":"applied","receipt":{"id":{"conversation_id":"original","ordinal":9007199254740993},"spec":{"type":"interval","every_seconds":60,"anchor_utc":"2026-01-01T00:00:00Z"}}}')?.id.ordinal).toBe('9007199254740993');
  expect(createdReceipt('{"status":"conflict"}')).toBeUndefined();
});
it('opens a canonical creation card through its original task binding, independently of the displayed node', async () => {
  const f = await fixture(), openTask = vi.fn(), cut = checkpoint(snapshot()).read_cut;
  const result = { status: { type: 'success' as const }, duration_ms: '0', content: [{ type: 'json' as const, value: { status: 'applied', receipt: { id: task().id, spec: task().spec } } }] };
  const tool: ForegroundToolExecution = { occurrence: { locator: { type: 'by_message_id', message_id: 'create-task' }, block_index: 0 },
    block_index: 0, assembly_complete: true, name: 'schedule_command', tool_id: 'native.schedule_command', committed_result: result, committed_result_deferred: false,
    state: { type: 'settled', arguments: JSON.stringify({ mutation: { type: 'create', title: task().title } }), result } };
  f.server.handlers.set('session/tracePage', request => {
    if (request.method !== 'session/tracePage') throw Error('original Tool');
    return { type: 'checkpoint_page', page: { read_cut: cut, cursor: cut.revision, domain: request.params.domain, total: '1',
      items: [{ offset: '0', identity: 'create-task', value: tool as never, deferred: false, content: [] }], collections: [], next_offset: null } };
  });
  render(<TaskNavigationProvider value={{ openTask, openTasks: vi.fn() }}><JournalSourceProvider source={{ client: f.server.client, sessionId: 'A', nodeId: nodeB, cut: () => cut, owned: () => true }}><CreatedTasks tools={[tool]}/></JournalSourceProvider></TaskNavigationProvider>);
  const button = await screen.findByRole('button', { name: 'Open task' }); await waitFor(() => expect((button as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(button); expect(openTask).toHaveBeenCalledWith(taskRef(task()), task().title); expect(f.server.claims()).toEqual([]);
});
