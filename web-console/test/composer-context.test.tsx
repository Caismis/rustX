import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { GoalSnapshot, RuntimeClientSnapshot, TodoTask } from '../../protocol/app-server/v4';
import { App } from '../src/app/App';
import { ComposerContextStack } from '../src/app/composer/ComposerContextStack';
import { GoalDock } from '../src/app/composer/GoalDock';
import { QueueDock } from '../src/app/composer/QueueDock';
import { TodoDock } from '../src/app/composer/TodoDock';
import { goalDock, queueRows, todoDock } from '../src/bindings/composer-context';
import { AppServerClient, type GoalControlOutcome } from '../src/client/app-server';
import { Server, snapshot } from './fixture';

const task = (id: string, status: TodoTask['status'], extra: Partial<TodoTask> = {}): TodoTask => ({ id, subject: `Task ${id}`, status, ...extra });
const goal = (extra: Partial<GoalSnapshot> = {}): GoalSnapshot => ({
  reference: { id: 'goal-1', revision: '3' }, objective: 'Ship the docks', phase: 'active',
  autonomous_round_budget: 4, autonomous_rounds_consumed: 1, origin: { kind: 'runtime_control' }, ...extra,
});
const withTodos = (tasks: TodoTask[] | undefined, base = snapshot()): RuntimeClientSnapshot => ({ ...base, todos: tasks && { tasks, next_id: String(tasks.length + 1) } });
const withGoal = (current: GoalSnapshot | null, armed = true, base = snapshot()): RuntimeClientSnapshot => ({ ...base, goal: { current, armed } });
const running = (base = snapshot()): RuntimeClientSnapshot => ({ ...base, attempt: { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 1 } });
const inbound = (sequence: string, text: string, message: { id?: string; source?: 'human' | 'runtime'; kind?: { goal_continuation: { id: string; revision: string } } } = {}) =>
  ({ sequence, message: { id: `message-${sequence}`, source: 'human' as const, content: [{ type: 'text' as const, text }], ...message } });
const withQueue = (rows: ReturnType<typeof inbound>[], base = snapshot()): RuntimeClientSnapshot => ({ ...base, inbound: { pending: rows } });
/** Historical execution facts only: a `todo` call and its committed result. */
const withTodoHistory = (base = snapshot()): RuntimeClientSnapshot => {
  const call = { role: 'assistant' as const, id: 'history-call', content: [{ type: 'tool_call' as const, id: 'call-todo', tool_id: 'tool-todo', name: 'todo', arguments: { action: 'create', subject: 'Historical task' } }] };
  const result = { role: 'tool' as const, id: 'history-result', tool_call_id: 'call-todo', tool_id: 'tool-todo', result: { status: { type: 'success' as const }, duration_ms: 1 } };
  return { ...base, transcript: { entries: [call, result].map((message, index) => ({ cursor: String(index + 1), item: { type: 'message' as const, message } })) } };
};

let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); });
async function mount(initial: RuntimeClientSnapshot, ...others: string[]) {
  server.snapshots.set('A', initial);
  await server.attached('A', ...others);
  localStorage.setItem('rustx-console-view-v1', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', tabs: ['A', ...others] }));
  return render(<App client={server.client} />);
}
const region = (name: 'To-dos' | 'Goal' | 'Queue') => screen.queryByRole('region', { name });
const dock = (name: 'To-dos' | 'Goal' | 'Queue') => screen.getByRole('region', { name });
const methods = () => server.requests.map(item => item.request.method);
const goalControls = () => server.requests.flatMap(item => item.request.method === 'goal/control' ? [item.request.params.control] : []);
const snapshotReads = () => methods().filter(method => method === 'session/snapshot').length;
const goalButton = (name: string) => within(dock('Goal')).getByRole('button', { name });
const CONFIGURATION_WRITES = ['settings/replace', 'settings/saveDefault', 'settings/setModel', 'settings/setApprovalMode', 'session/create'];
const update = (next: RuntimeClientSnapshot) => act(() => server.update('A', next));
/** Answer a held authoritative read with a transport-level failure. */
const failRead = (request: { id: string | number }) => act(async () => {
  server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'Snapshot unavailable' } });
});
/** A later native event whose successful snapshot read is the next authoritative observation. */
const recoverAuthority = async () => { server.held.delete('session/snapshot'); await update(server.snapshots.get('A')!); };
const sendQueued = async (text: string) => {
  fireEvent.change(screen.getByLabelText('Message'), { target: { value: text } });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Queue' })); });
};

describe('Todo dock binds only the composed native Todo projection', () => {
  it('distinguishes extension absent, composed empty and composed tasks', async () => {
    expect(todoDock(snapshot())).toEqual({ kind: 'absent' });
    expect(todoDock(withTodos([]))).toEqual({ kind: 'current', tasks: [] });
    await mount(snapshot());
    expect(region('To-dos')).toBeNull();
    await update(withTodos([]));
    expect(dock('To-dos').getAttribute('data-todo-state')).toBe('empty');
    expect(within(dock('To-dos')).getByText('No current tasks')).toBeTruthy();
    expect(within(dock('To-dos')).queryByRole('button')).toBeNull();
    await update(withTodos([task('1', 'completed'), task('2', 'in_progress', { active_form: 'Writing two' }), task('3', 'deleted'), task('4', 'pending', { blocked_by: ['2'] })]));
    const header = within(dock('To-dos')).getByRole('button', { expanded: false });
    expect(header.textContent).toContain('1 completed · 1 in progress · 1 pending');
    fireEvent.click(header);
    const rows = within(dock('To-dos')).getAllByRole('listitem');
    // Native order and statuses; the tombstone is not current work.
    expect(rows.map(row => [row.getAttribute('data-task-id'), row.getAttribute('data-status')])).toEqual([['1', 'completed'], ['2', 'in_progress'], ['4', 'pending']]);
    expect(rows[1].textContent).toContain('Writing two');
    expect(rows[2].textContent).toContain('after #2');
    expect(dock('To-dos').textContent).not.toContain('Task 3');
  });
  it('follows native projection changes and never reconstructs from historical todo Tool facts', async () => {
    await mount(withTodoHistory(snapshot()));
    // History is visible as execution fact, yet no Todo extension means no current dock.
    expect(screen.getByText('Tool call · call-todo')).toBeTruthy();
    expect(region('To-dos')).toBeNull();
    await update(withTodoHistory(withTodos([])));
    expect(dock('To-dos').getAttribute('data-todo-state')).toBe('empty');
    await update(withTodos([task('1', 'pending')]));
    expect(dock('To-dos').textContent).toContain('1 pending');
    await update(withTodoHistory(withTodos([task('1', 'completed')])));
    expect(dock('To-dos').textContent).toContain('1 completed');
    expect(dock('To-dos').textContent).not.toContain('Historical task');
  });
  it('owns no mutation and never writes current tasks into configuration or browser recovery storage', async () => {
    await mount(withTodos([task('1', 'pending', { subject: 'Current secret plan' })]));
    const before = methods().length;
    fireEvent.click(within(dock('To-dos')).getByRole('button', { expanded: false }));
    expect(within(dock('To-dos')).getAllByRole('button')).toHaveLength(1);
    expect(methods()).toHaveLength(before);
    expect(JSON.stringify(server.client.getSnapshot().views.A.settings)).not.toContain('Current secret plan');
    expect(JSON.stringify(localStorage)).not.toContain('Current secret plan');
  });
});

describe('Goal dock binds GoalDomain state and native goal/control', () => {
  it('renders active, paused, blocked; complete and absent take no space', async () => {
    expect(goalDock(snapshot())).toBeUndefined();
    expect(goalDock(withGoal(null))).toBeUndefined();
    expect(goalDock(withGoal(goal({ phase: 'complete' })))).toBeUndefined();
    await mount(withGoal(goal()));
    expect(dock('Goal').textContent).toContain('Ongoing Goal');
    expect(dock('Goal').textContent).toContain('Ship the docks');
    expect(dock('Goal').textContent).toContain('1/4 rounds · r3');
    expect(goalButton('Pause goal')).toBeTruthy();
    expect(within(dock('Goal')).queryByRole('button', { name: 'Resume goal' })).toBeNull();
    // Only controls GoalDomain assigns to users: no create, clear, complete or block.
    expect(within(dock('Goal')).getAllByRole('button').map(button => button.getAttribute('aria-label'))).toEqual(['Pause goal', 'Edit goal objective', 'Edit round budget']);
    await update(withGoal(goal({ phase: 'paused', reference: { id: 'goal-1', revision: '4' } }), false));
    expect(dock('Goal').textContent).toContain('Paused Goal');
    expect(goalButton('Resume goal')).toBeTruthy();
    await update(withGoal(goal({ phase: 'blocked', blocked_reason: 'Need repository access', reference: { id: 'goal-1', revision: '5' } }), false));
    expect(dock('Goal').textContent).toContain('Blocked Goal');
    expect(dock('Goal').textContent).toContain('Blocked: Need repository access');
    expect(goalButton('Resume goal')).toBeTruthy();
    await update(withGoal(goal({ phase: 'complete', reference: { id: 'goal-1', revision: '6' } }), false));
    expect(region('Goal')).toBeNull();
    await update(snapshot());
    expect(region('Goal')).toBeNull();
  });
  it('pause and resume use native goal/control with the rendered GoalRef and unlock after the reread', async () => {
    await mount(withGoal(goal()));
    fireEvent.click(goalButton('Pause goal'));
    await waitFor(() => expect(dock('Goal').textContent).toContain('Paused Goal'));
    expect(dock('Goal').textContent).toContain('r4');
    expect(goalButton('Resume goal')).toHaveProperty('disabled', false);
    fireEvent.click(goalButton('Resume goal'));
    await waitFor(() => expect(dock('Goal').textContent).toContain('Ongoing Goal'));
    expect(goalControls()).toEqual([
      { action: 'mutate', expected: { id: 'goal-1', revision: '3' }, mutation: { action: 'pause' } },
      { action: 'mutate', expected: { id: 'goal-1', revision: '4' }, mutation: { action: 'resume' } },
    ]);
    expect(methods().filter(method => CONFIGURATION_WRITES.includes(method))).toEqual([]);
    expect(JSON.stringify(server.client.getSnapshot().views.A.settings)).not.toMatch(/goal|Ship the docks/);
    expect(JSON.stringify(localStorage)).not.toContain('Ship the docks');
  });
  it('an applied control whose reread fails stays locked on the old GoalRef until a later authoritative read', async () => {
    await mount(withGoal(goal()));
    const reads = snapshotReads();
    server.held.add('session/snapshot');
    fireEvent.click(goalButton('Pause goal'));
    await failRead(await server.waitFor('session/snapshot', reads + 1));
    expect((await within(dock('Goal')).findByRole('status')).textContent).toContain('applied');
    // GoalDomain is at r4; the browser still renders its last authoritative observation.
    expect(dock('Goal').textContent).toContain('Ongoing Goal');
    expect(dock('Goal').textContent).toContain('r3');
    for (const name of ['Pause goal', 'Edit goal objective', 'Edit round budget']) expect(goalButton(name)).toHaveProperty('disabled', true);
    fireEvent.click(goalButton('Pause goal'));
    expect(goalControls()).toHaveLength(1);
    await recoverAuthority();
    await waitFor(() => expect(dock('Goal').textContent).toContain('Paused Goal'));
    expect(dock('Goal').textContent).toContain('r4');
    expect(goalButton('Resume goal')).toHaveProperty('disabled', false);
    expect(within(dock('Goal')).queryByRole('status')).toBeNull();
    expect(goalControls()).toHaveLength(1);
  });
  it('objective edit keeps its draft across revision-only change and sends the current revision', async () => {
    await mount(withGoal(goal()));
    fireEvent.click(goalButton('Edit goal objective'));
    const box = within(dock('Goal')).getByRole('textbox', { name: 'Goal objective' });
    expect(document.activeElement).toBe(box);
    fireEvent.change(box, { target: { value: 'Ship the docks today' } });
    // Autonomous admission advances revision and consumption only.
    await update(withGoal(goal({ reference: { id: 'goal-1', revision: '5' }, autonomous_rounds_consumed: 2 })));
    expect(within(dock('Goal')).getByRole('textbox', { name: 'Goal objective' })).toHaveProperty('value', 'Ship the docks today');
    fireEvent.keyDown(within(dock('Goal')).getByRole('textbox', { name: 'Goal objective' }), { key: 'Enter' });
    await waitFor(() => expect(dock('Goal').textContent).toContain('Ship the docks today'));
    expect(goalControls()).toEqual([{ action: 'mutate', expected: { id: 'goal-1', revision: '5' }, mutation: { action: 'edit', objective: 'Ship the docks today' } }]);
    await waitFor(() => expect(document.activeElement).toBe(goalButton('Edit goal objective')));
  });
  it('an authoritative objective change drops an open draft instead of writing over unseen content', async () => {
    await mount(withGoal(goal()));
    fireEvent.click(goalButton('Edit goal objective'));
    fireEvent.change(within(dock('Goal')).getByRole('textbox'), { target: { value: 'My draft' } });
    await update(withGoal(goal({ objective: 'Edited elsewhere', reference: { id: 'goal-1', revision: '4' } })));
    expect(within(dock('Goal')).queryByRole('textbox')).toBeNull();
    expect(dock('Goal').textContent).toContain('Edited elsewhere');
    expect(goalControls()).toEqual([]);
  });
  it('budget form checks only integer grammar; GoalDomain refuses out-of-domain values', async () => {
    expect(readFileSync(resolve(process.cwd(), 'src/app/composer/GoalDock.tsx'), 'utf8')).not.toMatch(/MAX_ROUND_BUDGET|autonomous_rounds_consumed\)/);
    await mount(withGoal(goal({ autonomous_rounds_consumed: 2 })));
    fireEvent.click(goalButton('Edit round budget'));
    const box = () => within(dock('Goal')).getByRole('spinbutton', { name: 'Autonomous round budget' });
    const save = () => goalButton('Save round budget');
    expect(box()).toHaveProperty('value', '4');
    for (const value of ['', '0', '2.5']) {
      fireEvent.change(box(), { target: { value } });
      expect(save()).toHaveProperty('disabled', true);
    }
    // Syntactically valid values beyond the native ceiling or below consumption reach the owner.
    for (const [value, attempt] of [['150', 1], ['1', 2]] as const) {
      fireEvent.change(box(), { target: { value } });
      expect(save()).toHaveProperty('disabled', false);
      fireEvent.keyDown(box(), { key: 'Enter' });
      expect((await within(dock('Goal')).findByRole('alert')).textContent).toBe('Invalid Goal transition or value');
      await waitFor(() => expect(save()).toHaveProperty('disabled', false));
      expect(goalControls()).toHaveLength(attempt);
    }
    fireEvent.change(box(), { target: { value: '6' } });
    fireEvent.keyDown(box(), { key: 'Enter' });
    await waitFor(() => expect(dock('Goal').textContent).toContain('2/6 rounds'));
    expect(goalControls().map(control => control.action === 'mutate' && [control.expected.revision, control.mutation])).toEqual([
      ['3', { action: 'budget', rounds: 150 }], ['3', { action: 'budget', rounds: 1 }], ['3', { action: 'budget', rounds: 6 }],
    ]);
    expect(methods().filter(method => CONFIGURATION_WRITES.includes(method))).toEqual([]);
  });
  it('a stale CAS refusal rereads authority, unlocks on the new observation and is never retried', async () => {
    await mount(withGoal(goal()));
    // A committed native write this client has not observed yet.
    server.snapshots.set('A', withGoal(goal({ reference: { id: 'goal-1', revision: '4' }, objective: 'Changed elsewhere' })));
    const reads = snapshotReads();
    fireEvent.click(goalButton('Pause goal'));
    expect((await within(dock('Goal')).findByRole('alert')).textContent).toBe('Stale GoalRef; observe current state before trying again');
    expect(dock('Goal').textContent).toContain('Changed elsewhere');
    expect(dock('Goal').textContent).toContain('Ongoing Goal');
    expect(dock('Goal').textContent).toContain('r4');
    expect(goalButton('Pause goal')).toHaveProperty('disabled', false);
    expect(snapshotReads()).toBe(reads + 1);
    expect(goalControls()).toEqual([{ action: 'mutate', expected: { id: 'goal-1', revision: '3' }, mutation: { action: 'pause' } }]);
  });
  it('a stale CAS refusal whose reread fails keeps the old GoalRef unusable until a later authoritative read', async () => {
    await mount(withGoal(goal()));
    server.snapshots.set('A', withGoal(goal({ reference: { id: 'goal-1', revision: '4' }, objective: 'Changed elsewhere' })));
    const reads = snapshotReads();
    server.held.add('session/snapshot');
    fireEvent.click(goalButton('Pause goal'));
    await failRead(await server.waitFor('session/snapshot', reads + 1));
    expect((await within(dock('Goal')).findByRole('alert')).textContent).toBe('Stale GoalRef; observe current state before trying again');
    expect(within(dock('Goal')).getByRole('status').textContent).toContain('locked');
    expect(dock('Goal').textContent).toContain('Ship the docks');
    expect(dock('Goal').textContent).toContain('r3');
    expect(goalButton('Pause goal')).toHaveProperty('disabled', true);
    fireEvent.click(goalButton('Pause goal'));
    expect(goalControls()).toHaveLength(1);
    await recoverAuthority();
    await waitFor(() => expect(dock('Goal').textContent).toContain('Changed elsewhere'));
    expect(dock('Goal').textContent).toContain('r4');
    expect(goalButton('Pause goal')).toHaveProperty('disabled', false);
    expect(goalControls()).toEqual([{ action: 'mutate', expected: { id: 'goal-1', revision: '3' }, mutation: { action: 'pause' } }]);
  });
  it('the dock itself locks after a known outcome without a reread until a newer observation arrives', async () => {
    const outcomes: GoalControlOutcome[] = [{ status: 'applied', observed: false }, { status: 'rejected', reason: 'Refused by GoalDomain', observed: false }];
    for (const outcome of outcomes) {
      const mutate = vi.fn(async () => outcome);
      const first = {};
      const props = { disabled: false, mutate };
      const ui = render(<GoalDock state={{ goal: goal(), armed: true }} observation={first} {...props} />);
      await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Pause goal' })); });
      expect(ui.getByRole('button', { name: 'Pause goal' })).toHaveProperty('disabled', true);
      if (outcome.status === 'rejected') expect(ui.getByRole('alert').textContent).toBe('Refused by GoalDomain');
      fireEvent.click(ui.getByRole('button', { name: 'Pause goal' }));
      ui.rerender(<GoalDock state={{ goal: goal(), armed: true }} observation={first} {...props} />);
      expect(ui.getByRole('button', { name: 'Pause goal' })).toHaveProperty('disabled', true);
      ui.rerender(<GoalDock state={{ goal: goal({ reference: { id: 'goal-1', revision: '4' } }), armed: true }} observation={{}} {...props} />);
      expect(ui.getByRole('button', { name: 'Pause goal' })).toHaveProperty('disabled', false);
      expect(ui.queryByRole('status')).toBeNull();
      expect(mutate).toHaveBeenCalledOnce();
      cleanup();
    }
  });
  it('activation-only change keeps the durable revision and an open draft', () => {
    const mutate = vi.fn(async (): Promise<GoalControlOutcome> => ({ status: 'applied', observed: true }));
    const current = goal();
    const ui = render(<GoalDock state={{ goal: current, armed: true }} observation={{}} disabled={false} mutate={mutate} />);
    fireEvent.click(ui.getByRole('button', { name: 'Edit goal objective' }));
    fireEvent.change(ui.getByRole('textbox'), { target: { value: 'Draft' } });
    ui.rerender(<GoalDock state={{ goal: current, armed: false }} observation={{}} disabled={false} mutate={mutate} />);
    expect(ui.getByRole('textbox')).toHaveProperty('value', 'Draft');
    fireEvent.keyDown(ui.getByRole('textbox'), { key: 'Escape' });
    expect(ui.container.textContent).toContain('Inactive Goal');
    expect(ui.container.textContent).toContain('r3');
    expect(ui.getByRole('button', { name: 'Resume goal' })).toBeTruthy();
    expect(ui.queryByRole('button', { name: 'Pause goal' })).toBeNull();
    expect(document.activeElement).toBe(ui.getByRole('button', { name: 'Edit goal objective' }));
    expect(mutate).not.toHaveBeenCalled();
  });
  it('a lost response stays uncertain until an authoritative reread and is not replayed', async () => {
    await mount(withGoal(goal()));
    server.held.add('goal/control');
    fireEvent.click(goalButton('Pause goal'));
    await act(async () => { await server.waitFor('goal/control', 1); });
    act(() => server.socket.close());
    expect((await within(dock('Goal')).findByRole('status')).textContent).toContain('uncertain');
    expect(goalButton('Pause goal')).toHaveProperty('disabled', true);
    await act(() => server.connect());
    await waitFor(() => expect(within(dock('Goal')).queryByRole('status')).toBeNull());
    expect(goalButton('Pause goal')).toHaveProperty('disabled', false);
    expect(dock('Goal').textContent).toContain('Ongoing Goal');
    expect(goalControls()).toHaveLength(1);
    expect(server.client.getSnapshot().uncertain.map(item => item.method)).toEqual(['goal/control']);
  });
  it('a control whose attachment changed settles as obsolete without touching the new view', async () => {
    server.snapshots.set('A', withGoal(goal()));
    await server.attached('A');
    server.held.add('goal/control');
    const work = server.client.controlGoal('A', goal().reference, { action: 'pause' });
    const request = await server.waitFor('goal/control', 1);
    await server.client.release('A', false); await server.client.attach('A');
    server.reply(request);
    expect(await work).toEqual({ status: 'obsolete' });
    expect(server.client.getSnapshot().views.A).toMatchObject({ attachment: 'attached', error: undefined });
    const mutate = vi.fn(async (): Promise<GoalControlOutcome> => ({ status: 'obsolete' }));
    const ui = render(<GoalDock state={{ goal: goal(), armed: true }} observation={{}} disabled={false} mutate={mutate} />);
    await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Pause goal' })); });
    expect(ui.queryByRole('alert')).toBeNull(); expect(ui.queryByRole('status')).toBeNull();
    expect(ui.getByRole('button', { name: 'Pause goal' })).toHaveProperty('disabled', false);
  });
});

describe('Queue dock binds the native inbound mailbox', () => {
  it('rows come from authoritative pending inbound in native sequence order', async () => {
    expect(queueRows(snapshot())).toEqual([]);
    await mount(running(withQueue([inbound('7', 'First queued'), inbound('8', 'Continue', { source: 'runtime', kind: { goal_continuation: { id: 'goal-1', revision: '2' } } })])));
    const header = within(dock('Queue')).getByRole('button', { expanded: false });
    expect(header.textContent).toContain('2 queued');
    fireEvent.click(header);
    const rows = within(dock('Queue')).getAllByRole('listitem');
    expect(rows.map(row => row.getAttribute('data-inbound-sequence'))).toEqual(['7', '8']);
    expect(rows[1].textContent).toContain('Goal continuation');
    // Basic dock only: no edit, remove or per-row steer (WEB-06).
    expect(within(dock('Queue')).getAllByRole('button')).toHaveLength(1);
    expect(Object.getOwnPropertyNames(AppServerClient.prototype).filter(name => /queue|inbox/i.test(name))).toEqual([]);
    await update(running(withQueue([inbound('8', 'Continue')])));
    expect(within(dock('Queue')).queryByRole('button')).toBeNull();
    expect(dock('Queue').textContent).toContain('#8');
    await update(running());
    expect(region('Queue')).toBeNull();
    expect(JSON.stringify(localStorage)).not.toContain('First queued');
    expect(JSON.stringify(server.client.getSnapshot().views.A.settings)).not.toContain('First queued');
  });
  it('composer delivery follows the authoritative attempt and uses the single native inbound method', async () => {
    await mount(snapshot());
    expect(screen.getByRole('button', { name: 'Send' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Steer' })).toBeNull();
    await update(running());
    expect(screen.getByRole('button', { name: 'Queue' })).toBeTruthy();
    expect(screen.getByText('Attempt running · Enter queues for its next safe boundary')).toBeTruthy();
    await sendQueued('While running');
    await server.waitFor('turn/start', 1);
    expect(methods()).not.toContain('turn/steer');
    await update(snapshot());
    expect(screen.getByRole('button', { name: 'Send' })).toBeTruthy();
  });
  it('an unacknowledged request is composer transport state, never a queue row', async () => {
    await mount(running(withQueue([inbound('1', 'Native row')])));
    server.held.add('turn/start');
    await sendQueued('Queued draft');
    const turn = await server.waitFor('turn/start', 1);
    expect(screen.getByRole('button', { name: 'Awaiting acknowledgement…' })).toBeTruthy();
    // No count header: the in-flight request is not counted as queued.
    expect(within(dock('Queue')).queryByRole('button')).toBeNull();
    expect(dock('Queue').querySelectorAll('li')).toHaveLength(1);
    expect(dock('Queue').querySelector('[data-submission-echo]')).toBeNull();
    expect(dock('Queue').textContent).not.toContain('Queued draft');
    expect(server.client.getSnapshot().views.A.submissions ?? []).toEqual([]);
    await act(async () => { server.reply(turn); });
    // Acceptance names the server MessageId; only now may provisional presentation exist.
    const header = await within(dock('Queue')).findByRole('button', { expanded: false });
    expect(header.textContent).toContain('2 queued');
    expect(within(header).getByRole('status').textContent).toBe('1 accepted · awaiting projection');
    fireEvent.click(header);
    const echo = dock('Queue').querySelector('[data-submission-echo]')!;
    expect(echo.getAttribute('data-accepted-message-id')).toBe('accepted-user');
    expect(echo.textContent).toContain('Accepted · awaiting projection');
    expect(server.client.getSnapshot().views.A.submissions?.map(item => item.messageId)).toEqual(['accepted-user']);
  });
  it('an accepted echo settles only by its exact MessageId', async () => {
    await mount(running());
    await sendQueued('Queued draft');
    await waitFor(() => expect(dock('Queue').querySelector('[data-accepted-message-id="accepted-user"]')).toBeTruthy());
    // Identical text under another identity is not this submission.
    await update(running(withQueue([inbound('2', 'Queued draft', { id: 'someone-else' })])));
    fireEvent.click(within(dock('Queue')).getByRole('button', { expanded: false }));
    expect(dock('Queue').querySelectorAll('[data-submission-echo]')).toHaveLength(1);
    await update(running(withQueue([inbound('2', 'Queued draft', { id: 'someone-else' }), inbound('3', 'Queued draft', { id: 'accepted-user' })])));
    expect(dock('Queue').querySelectorAll('[data-submission-echo]')).toHaveLength(0);
    expect([...dock('Queue').querySelectorAll('[data-message-id]')].map(row => row.getAttribute('data-message-id'))).toEqual(['someone-else', 'accepted-user']);
  });
  it('a lost acknowledgement leaves only the uncertain diagnostic; reconnect repairs from authority without replay', async () => {
    await mount(running());
    server.held.add('turn/start');
    await sendQueued('Lost acknowledgement');
    await server.waitFor('turn/start', 1);
    act(() => server.socket.close());
    expect(region('Queue')).toBeNull();
    expect(server.client.getSnapshot().uncertain.map(item => item.method)).toEqual(['turn/start']);
    expect(screen.getByText('Outcome uncertain: turn/start')).toBeTruthy();
    server.held.delete('turn/start');
    // The runtime did commit it; only an authoritative read may say so.
    server.snapshots.set('A', running(withQueue([inbound('5', 'Lost acknowledgement', { id: 'accepted-user' })])));
    await act(() => server.connect());
    await waitFor(() => expect(dock('Queue').querySelector('[data-message-id="accepted-user"]')).toBeTruthy());
    expect(dock('Queue').querySelector('[data-submission-echo]')).toBeNull();
    expect(methods().filter(method => method === 'turn/start')).toHaveLength(1);
  });
});

describe('Composer context stack lifecycle', () => {
  it('orders Todo, Goal, Queue, Composer and each card appears independently without state leakage', async () => {
    const full = running(withQueue([inbound('1', 'one'), inbound('2', 'two')], withGoal(goal(), true, withTodos([task('1', 'pending')]))));
    const ui = await mount(full);
    const order = () => [...ui.container.querySelector('[data-composer-context-stack]')!.children].map(node => node.getAttribute('aria-label') ?? (node.querySelector('[data-composer-card]') ? 'Composer' : 'unknown'));
    expect(order()).toEqual(['To-dos', 'Goal', 'Queue', 'Composer']);
    fireEvent.click(within(dock('To-dos')).getByRole('button', { expanded: false }));
    fireEvent.click(goalButton('Edit goal objective'));
    fireEvent.change(within(dock('Goal')).getByRole('textbox'), { target: { value: 'Kept draft' } });
    // Queue disappears: Todo disclosure and Goal draft are unaffected.
    await update(running(withGoal(goal(), true, withTodos([task('1', 'pending')]))));
    expect(order()).toEqual(['To-dos', 'Goal', 'Composer']);
    expect(within(dock('To-dos')).getByRole('button', { expanded: true })).toBeTruthy();
    expect(within(dock('Goal')).getByRole('textbox')).toHaveProperty('value', 'Kept draft');
    // Todo disappears and Queue returns collapsed; Goal keeps its own draft.
    await update(running(withQueue([inbound('3', 'three'), inbound('4', 'four')], withGoal(goal()))));
    expect(order()).toEqual(['Goal', 'Queue', 'Composer']);
    expect(within(dock('Queue')).getByRole('button', { expanded: false })).toBeTruthy();
    expect(within(dock('Goal')).getByRole('textbox')).toHaveProperty('value', 'Kept draft');
    await update(withTodos([task('1', 'pending')]));
    expect(order()).toEqual(['To-dos', 'Composer']);
    expect(within(dock('To-dos')).getByRole('button', { expanded: false })).toBeTruthy();
  });
  it('the stack composes fixed seats regardless of which docks render', () => {
    const ui = render(<ComposerContextStack todo={<TodoDock state={{ kind: 'current', tasks: [] }} />} goal={null}
      queue={<QueueDock rows={[inbound('1', 'row')]} submissions={[]} running={false} />} composer={<div data-composer-card />} />);
    expect([...ui.container.firstElementChild!.children].map(node => node.getAttribute('aria-label') ?? 'Composer')).toEqual(['To-dos', 'Queue', 'Composer']);
  });
  it('Session views never share dock presentation state', async () => {
    server.snapshots.set('B', withTodos([task('1', 'pending')], snapshot('B')));
    await mount(withTodos([task('1', 'pending')]), 'B');
    fireEvent.click(within(dock('To-dos')).getByRole('button', { expanded: false }));
    fireEvent.click(screen.getByRole('tab', { name: 'Session B' }));
    expect(within(dock('To-dos')).getByRole('button', { expanded: false })).toBeTruthy();
  });
  it('disconnect clears only accepted presentation echoes; reconnect rebuilds docks from the new authoritative snapshot', async () => {
    await mount(running(withQueue([inbound('1', 'Pending before loss')], withGoal(goal(), true, withTodos([task('1', 'in_progress')])))));
    await sendQueued('Echo only');
    const header = await within(dock('Queue')).findByRole('button', { expanded: false });
    fireEvent.click(header);
    expect(dock('Queue').querySelectorAll('[data-submission-echo]')).toHaveLength(1);
    const before = methods().length;
    act(() => server.socket.close());
    // Last observations stay visible but inert; nothing was cancelled, disarmed or settled.
    expect(dock('Queue').querySelectorAll('[data-submission-echo]')).toHaveLength(0);
    expect(dock('Queue').textContent).toContain('Pending before loss');
    expect(dock('To-dos').textContent).toContain('1 in progress');
    expect(goalButton('Pause goal')).toHaveProperty('disabled', true);
    expect(methods().slice(before)).toEqual([]);
    // Native state moved on while this browser was away.
    server.snapshots.set('A', withGoal(goal({ phase: 'paused', reference: { id: 'goal-1', revision: '9' } }), false, withTodos([task('1', 'completed')])));
    await act(() => server.connect());
    await waitFor(() => expect(dock('Goal').textContent).toContain('Paused Goal'));
    expect(dock('Goal').textContent).toContain('r9');
    expect(dock('To-dos').textContent).toContain('1 completed');
    expect(region('Queue')).toBeNull();
    expect(methods().filter(method => ['goal/control', 'turn/cancel', 'turn/steer'].includes(method))).toEqual([]);
  });
  it('every dock shares the composer card column without viewport positioning', () => {
    const composer = (name: string) => readFileSync(resolve(process.cwd(), 'src/app/composer', name), 'utf8');
    for (const name of ['TodoDock', 'GoalDock', 'QueueDock']) {
      const source = composer(`${name}.module.css`);
      expect(source).toContain('width: calc(100% - 2 * var(--dsh-composer-side-clearance) - 4 * var(--dsh-composer-dock-inset));');
      expect(source).toContain('max-width: calc(var(--dsh-composer-card-max-width) - 4 * var(--dsh-composer-dock-inset));');
      expect(source).not.toMatch(/position:\s*(fixed|sticky)/);
    }
    expect(composer('ComposerContextStack.module.css')).toContain('--dsh-composer-dock-inset: 8px;');
  });
});
