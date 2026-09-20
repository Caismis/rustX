import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { GoalSnapshot, RuntimeClientSnapshot, TodoTask } from '../../protocol/app-server/v13';
import { App } from '../src/app/App';
import { ComposerContextStack } from '../src/app/composer/ComposerContextStack';
import { GoalDock } from '../src/app/composer/GoalDock';
import { QueueDock } from '../src/app/composer/QueueDock';
import { TodoDock } from '../src/app/composer/TodoDock';
import { goalDock, queueRows, todoDock, type TodoDockState } from '../src/bindings/composer-context';
import { AppServerClient, type GoalControlOutcome } from '../src/client/app-server';
import { Server, snapshot } from './fixture';

const task = (id: string, status: TodoTask['status'], extra: Partial<TodoTask> = {}): TodoTask => ({ id, subject: `Task ${id}`, status, ...extra });
const goal = (extra: Partial<GoalSnapshot> = {}): GoalSnapshot => ({
  reference: { id: 'goal-1', revision: '3' }, objective: 'Ship the docks', phase: 'active',
  autonomous_round_budget: 4, autonomous_rounds_consumed: 1, origin: { kind: 'runtime_control' }, ...extra,
});
const withTodos = (tasks: TodoTask[] | undefined, base = snapshot()): RuntimeClientSnapshot => ({ ...base, todos: tasks && { tasks, next_id: String(tasks.length + 1) } });
const withGoal = (current: GoalSnapshot | null, base = snapshot()): RuntimeClientSnapshot => ({ ...base, goal: { current } });
const running = (base = snapshot()): RuntimeClientSnapshot => ({ ...base, attempt: { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 1 } });
const inbound = (sequence: string, text: string, message: { id?: string; source?: 'human' | 'runtime'; kind?: { goal_continuation: { id: string; revision: string } } } = {}) =>
  ({ revision: "0", sequence, message: { id: `message-${sequence}`, source: 'human' as const, content: [{ type: 'text' as const, text }], ...message } });
const withQueue = (rows: ReturnType<typeof inbound>[], base = snapshot()): RuntimeClientSnapshot => ({ ...base, inbound: { pending: rows } });
/** One anchored historical Agent Status composition carrying Todo A, on a
 * conversation whose later turn must never receive it. */
const withStatusHistory = (base = snapshot()): RuntimeClientSnapshot => {
  const history = withTodoHistory(base);
  const entries = [
    { cursor: '0', item: { type: 'message' as const, message: { role: 'user' as const, id: 'u1', source: 'human' as const, content: [{ type: 'text' as const, text: 'Plan the work' }] } } },
    ...history.transcript.entries!,
    { cursor: '3', item: { type: 'message' as const, message: { role: 'assistant' as const, id: 'a1', content: [{ type: 'text' as const, text: 'Planned.' }] } } },
    { cursor: '4', item: { type: 'message' as const, message: { role: 'user' as const, id: 'u2', source: 'human' as const, content: [{ type: 'text' as const, text: 'Anything else?' }] } } },
  ];
  return { ...history, transcript: { entries }, statuses: [{
    attempt_id: 'attempt-A', turn: 1, status_message_id: 'status-1', rendered: 'historical rendered prose',
    opportunities: { fresh_inbound: { target_message_id: 'u1' } },
    sections: [{ type: 'todo', current: { id: '1', subject: 'Todo A', status: 'in_progress', blocked: false, active_form: 'Doing Todo A' }, tasks: [], active_count: 1, blocked_count: 0, completed_count: 0, deleted_count: 0, omitted_count: 0 }],
  }] };
};

/** Historical execution facts only: a `todo` call and its committed result. */
const withTodoHistory = (base = snapshot()): RuntimeClientSnapshot => {
  const call = { role: 'assistant' as const, id: 'history-call', content: [{ type: 'tool_call' as const, id: 'call-todo', tool_id: 'tool-todo', name: 'todo', arguments: { action: 'create', subject: 'Historical task' } }] };
  const result = { role: 'tool' as const, occurrence: { assistant_message_id: 'history-call', block_index: 0 }, id: 'history-result', tool_call_id: 'call-todo', tool_id: 'tool-todo', result: { status: { type: 'success' as const }, duration_ms: 1 } };
  return { ...base, transcript: { entries: [call, result].map((message, index) => ({ cursor: String(index + 1), item: { type: 'message' as const, message } })) } };
};

let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); });
async function mount(initial: RuntimeClientSnapshot, ...others: string[]) {
  server.snapshots.set('A', initial);
  await server.attached('A', ...others);
  localStorage.setItem('rustx-console-view-v2', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A', ...others] }));
  return render(<App client={server.client} workspaceHost={server.workspaceHost} />);
}
const region = (name: 'To-dos' | 'Goal' | 'Queue') => screen.queryByRole('region', { name });
const dock = (name: 'To-dos' | 'Goal' | 'Queue') => screen.getByRole('region', { name });
const methods = () => server.requests.map(item => item.request.method);
const goalControls = () => server.requests.flatMap(item => item.request.method === 'goal/control' ? [item.request.params.control] : []);
const snapshotReads = () => methods().filter(method => method === 'session/snapshot').length;
const goalButton = (name: string) => within(dock('Goal')).getByRole('button', { name });
const CONFIGURATION_WRITES = ['settings/replace', 'configuration/sourceWrite', 'configuration/reload', 'settings/setModel', 'session/create'];
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
  it('R08/R09: absent, composed-empty and deleted-only render no dock; a mixed list is collapsed with non-zero counts', async () => {
    // The two native facts stay distinct in the binding and share one visual result.
    expect(todoDock(snapshot())).toEqual({ kind: 'absent' });
    expect(todoDock(withTodos([]))).toEqual({ kind: 'current', tasks: [] });
    expect(todoDock(withTodos([task('1', 'deleted')]))).toEqual({ kind: 'current', tasks: [] });
    await mount(snapshot());
    expect(region('To-dos')).toBeNull();
    await update(withTodos([]));
    expect(region('To-dos')).toBeNull();
    // The obsolete empty strip is gone: no placeholder, no Todo-specific chrome.
    expect(screen.queryByText('No current tasks')).toBeNull();
    expect(document.querySelector('[data-todo-state]')).toBeNull();
    await update(withTodos([task('1', 'deleted'), task('2', 'deleted')]));
    expect(region('To-dos')).toBeNull();
    expect(document.querySelector('[data-todo-state]')).toBeNull();
    await update(withTodos([task('1', 'completed'), task('2', 'in_progress', { active_form: 'Writing two' }), task('3', 'deleted'), task('4', 'pending', { blocked_by: ['2'] })]));
    const header = within(dock('To-dos')).getByRole('button', { expanded: false });
    expect(header.textContent).toContain(`1 completed · 1 in progress · 1 pending`);
    // The collapsed header is a stable count summary, never an activity ticker.
    expect(header.textContent).not.toContain('Writing two');
    expect(header.getAttribute('aria-controls')).toBeNull();
    fireEvent.click(header);
    const list = within(dock('To-dos')).getByRole('list');
    expect(header.getAttribute('aria-controls')).toBe(list.id);
    const rows = within(dock('To-dos')).getAllByRole('listitem');
    // Native order and statuses; the tombstone is not current work.
    expect(rows.map(row => [row.getAttribute('data-task-id'), row.getAttribute('data-status')])).toEqual([['1', 'completed'], ['2', 'in_progress'], ['4', 'pending']]);
    expect(rows[1].textContent).toContain('Writing two');
    // A dependency is secondary relation text, not a fourth status.
    expect(rows[2].textContent).toContain('after #2');
    expect(rows[2].getAttribute('data-status')).toBe('pending');
    expect(dock('To-dos').textContent).not.toContain('Task 3');
  });
  it('R09: every parallel in-progress task is counted and rendered, with active_form only where the runtime published one', async () => {
    await mount(withTodos([task('1', 'completed'), task('2', 'in_progress', { active_form: 'Writing two' }), task('3', 'in_progress'), task('4', 'in_progress', { active_form: 'Reading four' }), task('5', 'pending')]));
    const header = within(dock('To-dos')).getByRole('button', { expanded: false });
    expect(header.textContent).toContain(`1 completed · 3 in progress · 1 pending`);
    fireEvent.click(header);
    const rows = within(dock('To-dos')).getAllByRole('listitem');
    expect(rows.filter(row => row.getAttribute('data-status') === 'in_progress')).toHaveLength(3);
    // An in-progress task without active_form keeps its subject; counts and rows
    // describe exactly the same filtered current list.
    expect(rows.map(row => row.querySelector('[title]')?.getAttribute('title'))).toEqual(['Task 1', 'Writing two', 'Task 3', 'Reading four', 'Task 5']);
  });
  it('R09: the disclosure is keyboard operable and keeps accurate accessible relationships', async () => {
    await mount(withTodos([task('1', 'pending')]));
    const header = within(dock('To-dos')).getByRole('button', { expanded: false });
    // A native button carries the accessible name, Enter/Space activation and focus
    // behaviour without a key handler or a role of its own.
    expect(header.tagName).toBe('BUTTON');
    header.focus();
    expect(document.activeElement).toBe(header);
    fireEvent.click(header);
    expect(within(dock('To-dos')).getByRole('button', { expanded: true })).toBe(header);
    expect(document.activeElement).toBe(header);
    expect(within(dock('To-dos')).getByRole('list').id).toBe(header.getAttribute('aria-controls'));
  });
  it('R10: an all-completed list is not an empty list, and turn boundaries never clear the dock', async () => {
    await mount(withTodos([task('1', 'completed'), task('2', 'completed')]));
    const header = within(dock('To-dos')).getByRole('button', { expanded: false });
    expect(header.textContent).toContain('2 completed');
    expect(header.textContent).not.toMatch(/in progress|pending/);
    // Unchanged native Todo: neither a started nor a settled attempt clears it.
    await update(running(withTodos([task('1', 'completed'), task('2', 'completed')])));
    expect(dock('To-dos').textContent).toContain('2 completed');
    await update(withTodos([task('1', 'completed'), task('2', 'completed')]));
    expect(dock('To-dos').textContent).toContain('2 completed');
    // Only the authoritative current list actually becoming empty removes it.
    await update(withTodos([]));
    expect(region('To-dos')).toBeNull();
  });
  it('R11: disclosure resets when the visible list disappears and never on ordinary non-empty updates', async () => {
    await mount(withTodos([task('1', 'pending'), task('2', 'pending')]));
    fireEvent.click(within(dock('To-dos')).getByRole('button', { expanded: false }));
    // A task completing is an ordinary update; an open list stays open.
    await update(withTodos([task('1', 'completed'), task('2', 'pending')]));
    expect(within(dock('To-dos')).getByRole('button', { expanded: true })).toBeTruthy();
    await update(withTodos([task('1', 'completed'), task('2', 'in_progress'), task('3', 'pending')]));
    expect(within(dock('To-dos')).getByRole('button', { expanded: true })).toBeTruthy();
    await update(withTodos([]));
    expect(region('To-dos')).toBeNull();
    // A later native list is a new presentation, not a restored one.
    await update(withTodos([task('9', 'pending')]));
    expect(within(dock('To-dos')).getByRole('button', { expanded: false })).toBeTruthy();
    // Extension absence resets it too.
    fireEvent.click(within(dock('To-dos')).getByRole('button', { expanded: false }));
    await update(snapshot());
    expect(region('To-dos')).toBeNull();
    await update(withTodos([task('9', 'pending')]));
    expect(within(dock('To-dos')).getByRole('button', { expanded: false })).toBeTruthy();
  });
  it('follows native projection changes and never reconstructs from historical todo Tool facts', async () => {
    await mount(withTodoHistory(snapshot()));
    // History is visible as execution fact, yet no Todo extension means no current dock.
    expect(screen.getByText('Assembling todo…')).toBeTruthy();
    expect(region('To-dos')).toBeNull();
    await update(withTodoHistory(withTodos([])));
    expect(region('To-dos')).toBeNull();
    await update(withTodos([task('1', 'pending')]));
    expect(dock('To-dos').textContent).toContain('1 pending');
    await update(withTodoHistory(withTodos([task('1', 'completed')])));
    expect(dock('To-dos').textContent).toContain('1 completed');
    expect(dock('To-dos').textContent).not.toContain('Historical task');
  });
  it('R07: clearing current Todo retires the dock while the historical Agent Status annotation stays at its anchor', async () => {
    // Todo A is actionable and one composed Agent Status recorded it at its turn.
    const actionable = [task('1', 'in_progress', { subject: 'Todo A', active_form: 'Doing Todo A' })];
    await mount(withStatusHistory(withTodos(actionable)));
    fireEvent.click(within(dock('To-dos')).getByRole('button', { expanded: false }));
    expect(dock('To-dos').textContent).toContain('Doing Todo A');
    expect(screen.getByRole('note', { name: 'Agent Status' }).closest('[data-chat-anchor-key]')?.getAttribute('data-chat-anchor-key')).toBe('message:u1');
    // Todo clear: the authoritative current list is empty and, being non-actionable,
    // emits no new Todo Agent Status. The older composition remains a historical fact.
    await update(withStatusHistory(withTodos([])));
    expect(region('To-dos')).toBeNull();
    const notes = screen.getAllByRole('note', { name: 'Agent Status' });
    expect(notes).toHaveLength(1);
    expect(notes[0].getAttribute('data-agent-status')).toBe('status-1');
    expect(notes[0].closest('[data-chat-anchor-key]')?.getAttribute('data-chat-anchor-key')).toBe('message:u1');
    // The latest messages receive nothing, and the historical Tool evidence survives.
    for (const key of ['message:a1', 'message:u2']) {
      expect(within(document.querySelector(`[data-chat-anchor-key="${key}"]`) as HTMLElement).queryByRole('note')).toBeNull();
    }
    expect(screen.getByText('Assembling todo…')).toBeTruthy();
    // The historical annotation still carries the historical Todo section, and no
    // current Todo selector ever reads an Agent Status section.
    fireEvent.click(within(notes[0]).getByRole('button', { expanded: false }));
    expect(notes[0].textContent).toContain('Doing Todo A');
    expect(todoDock(withStatusHistory(withTodos([])))).toEqual({ kind: 'current', tasks: [] });
    expect(todoDock(withStatusHistory(snapshot()))).toEqual({ kind: 'absent' });
    expect(region('To-dos')).toBeNull();
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
    expect(dock('Goal').textContent).toContain('Active Goal');
    expect(dock('Goal').textContent).toContain('Ship the docks');
    expect(dock('Goal').textContent).toContain('1/4 rounds');
    expect(goalButton('Pause goal')).toBeTruthy();
    expect(within(dock('Goal')).queryByRole('button', { name: 'Resume goal' })).toBeNull();
    // Only controls GoalDomain assigns to users: no create, clear, complete or block.
    expect(within(dock('Goal')).getAllByRole('button').map(button => button.getAttribute('aria-label'))).toEqual(['Pause goal', 'Edit goal objective', 'Edit round budget']);
    await update(withGoal(goal({ phase: 'paused', reference: { id: 'goal-1', revision: '4' } })));
    expect(dock('Goal').textContent).toContain('Paused Goal');
    expect(goalButton('Resume goal')).toBeTruthy();
    await update(withGoal(goal({ phase: 'blocked', blocked_reason: 'Need repository access', reference: { id: 'goal-1', revision: '5' } })));
    expect(dock('Goal').textContent).toContain('Blocked Goal');
    expect(dock('Goal').textContent).toContain('Blocked: Need repository access');
    expect(goalButton('Resume goal')).toBeTruthy();
    await update(withGoal(goal({ phase: 'complete', reference: { id: 'goal-1', revision: '6' } })));
    expect(region('Goal')).toBeNull();
    await update(snapshot());
    expect(region('Goal')).toBeNull();
  });
  it('pause and resume use native goal/control with the rendered GoalRef and unlock after the reread', async () => {
    await mount(withGoal(goal()));
    fireEvent.click(goalButton('Pause goal'));
    await waitFor(() => expect(dock('Goal').textContent).toContain('Paused Goal'));
    expect(server.client.getSnapshot().views.A.snapshot?.goal?.current?.reference.revision).toBe('4');
    // The projected text and the control lock are two different signals: this
    // text is published *by* the reread, and the dock unlocks only once the
    // mutation itself settles afterwards. Gate on the unlock, never on its
    // proxy, or the assertion lands inside that window.
    await waitFor(() => expect(goalButton('Resume goal')).toHaveProperty('disabled', false));
    fireEvent.click(goalButton('Resume goal'));
    await waitFor(() => expect(dock('Goal').textContent).toContain('Active Goal'));
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
    expect(dock('Goal').textContent).toContain('Active Goal');
    expect(server.client.getSnapshot().views.A.snapshot?.goal?.current?.reference.revision).toBe('3');
    for (const name of ['Pause goal', 'Edit goal objective', 'Edit round budget']) expect(goalButton(name)).toHaveProperty('disabled', true);
    fireEvent.click(goalButton('Pause goal'));
    expect(goalControls()).toHaveLength(1);
    await recoverAuthority();
    await waitFor(() => expect(dock('Goal').textContent).toContain('Paused Goal'));
    expect(server.client.getSnapshot().views.A.snapshot?.goal?.current?.reference.revision).toBe('4');
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
  it('a budget the protocol u32 cannot represent never reaches the wire; u32::MAX does', async () => {
    await mount(withGoal(goal({ autonomous_rounds_consumed: 2 })));
    fireEvent.click(goalButton('Edit round budget'));
    const box = () => within(dock('Goal')).getByRole('spinbutton', { name: 'Autonomous round budget' });
    const save = () => goalButton('Save round budget');
    // Grammar failures, and positive integers beyond the wire `u32`, are refused
    // locally. The huge decimal must not round, reach Infinity or become JSON null.
    for (const value of ['', '0', '2.5', '1e3', '+12', '04', '4294967296', '4294967300', '9'.repeat(40), '1'.repeat(400)]) {
      fireEvent.change(box(), { target: { value } });
      expect(save()).toHaveProperty('disabled', true);
      fireEvent.keyDown(box(), { key: 'Enter' });
    }
    expect(goalControls()).toEqual([]);
    expect(methods()).not.toContain('goal/control');
    // The exact maximum is representable: the browser sends it and the owner refuses it.
    fireEvent.change(box(), { target: { value: '4294967295' } });
    expect(save()).toHaveProperty('disabled', false);
    fireEvent.keyDown(box(), { key: 'Enter' });
    expect((await within(dock('Goal')).findByRole('alert')).textContent).toBe('Invalid Goal transition or value');
    expect(goalControls()).toEqual([{ action: 'mutate', expected: { id: 'goal-1', revision: '3' }, mutation: { action: 'budget', rounds: 4294967295 } }]);
    // Serialized as the exact integer, never a rounded value or null.
    const sent = server.requests.filter(item => item.request.method === 'goal/control').at(-1)!.request;
    expect(JSON.stringify(sent)).toContain('"rounds":4294967295');
  });
  it('a stale CAS refusal rereads authority, unlocks on the new observation and is never retried', async () => {
    await mount(withGoal(goal()));
    // A committed native write this client has not observed yet.
    server.snapshots.set('A', withGoal(goal({ reference: { id: 'goal-1', revision: '4' }, objective: 'Changed elsewhere' })));
    const reads = snapshotReads();
    fireEvent.click(goalButton('Pause goal'));
    expect((await within(dock('Goal')).findByRole('alert')).textContent).toBe('Stale GoalRef; observe current state before trying again');
    expect(dock('Goal').textContent).toContain('Changed elsewhere');
    expect(dock('Goal').textContent).toContain('Active Goal');
    expect(server.client.getSnapshot().views.A.snapshot?.goal?.current?.reference.revision).toBe('4');
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
    expect(server.client.getSnapshot().views.A.snapshot?.goal?.current?.reference.revision).toBe('3');
    expect(goalButton('Pause goal')).toHaveProperty('disabled', true);
    fireEvent.click(goalButton('Pause goal'));
    expect(goalControls()).toHaveLength(1);
    await recoverAuthority();
    await waitFor(() => expect(dock('Goal').textContent).toContain('Changed elsewhere'));
    expect(server.client.getSnapshot().views.A.snapshot?.goal?.current?.reference.revision).toBe('4');
    expect(goalButton('Pause goal')).toHaveProperty('disabled', false);
    expect(goalControls()).toEqual([{ action: 'mutate', expected: { id: 'goal-1', revision: '3' }, mutation: { action: 'pause' } }]);
  });
  it('the dock itself locks after a known outcome without a reread until a newer observation arrives', async () => {
    const outcomes: GoalControlOutcome[] = [{ status: 'applied', observed: false }, { status: 'rejected', reason: 'Refused by GoalDomain', observed: false }];
    for (const outcome of outcomes) {
      const mutate = vi.fn(async () => outcome);
      const first = {};
      const props = { disabled: false, mutate };
      const ui = render(<GoalDock state={{ goal: goal() }} observation={first} {...props} />);
      await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Pause goal' })); });
      expect(ui.getByRole('button', { name: 'Pause goal' })).toHaveProperty('disabled', true);
      if (outcome.status === 'rejected') expect(ui.getByRole('alert').textContent).toBe('Refused by GoalDomain');
      fireEvent.click(ui.getByRole('button', { name: 'Pause goal' }));
      ui.rerender(<GoalDock state={{ goal: goal() }} observation={first} {...props} />);
      expect(ui.getByRole('button', { name: 'Pause goal' })).toHaveProperty('disabled', true);
      ui.rerender(<GoalDock state={{ goal: goal({ reference: { id: 'goal-1', revision: '4' } }) }} observation={{}} {...props} />);
      expect(ui.getByRole('button', { name: 'Pause goal' })).toHaveProperty('disabled', false);
      expect(ui.queryByRole('status')).toBeNull();
      expect(mutate).toHaveBeenCalledOnce();
      cleanup();
    }
  });
  // Issue #351: the status and the one lifecycle control both derive from the
  // durable phase. Active offers Pause and nothing else; Paused and Blocked
  // offer Resume. "Inactive Goal" no longer exists, and no snapshot can spell
  // an Active Goal that is not actually running.
  it('status and the single lifecycle control derive from durable phase alone', () => {
    const mutate = vi.fn(async (): Promise<GoalControlOutcome> => ({ status: 'applied', observed: true }));
    for (const [phase, status, control, other] of [
      ['active', 'Active Goal', 'Pause goal', 'Resume goal'],
      ['paused', 'Paused Goal', 'Resume goal', 'Pause goal'],
      ['blocked', 'Blocked Goal', 'Resume goal', 'Pause goal'],
    ] as const) {
      const ui = render(<GoalDock state={{ goal: goal({ phase }) }} observation={{}} disabled={false} mutate={mutate} />);
      expect(ui.container.textContent).toContain(status);
      expect(ui.container.textContent).not.toContain('Inactive');
      expect(ui.getByRole('button', { name: control })).toBeTruthy();
      expect(ui.queryByRole('button', { name: other })).toBeNull();
      expect(ui.container.querySelector('[data-goal-armed]')).toBeNull();
      expect(ui.container.querySelector(`[data-goal-phase="${phase}"]`)).toBeTruthy();
      cleanup();
    }
    expect(mutate).not.toHaveBeenCalled();
  });
  it('an autonomous round advances the revision without disturbing an open draft', () => {
    const mutate = vi.fn(async (): Promise<GoalControlOutcome> => ({ status: 'applied', observed: true }));
    const current = goal();
    const ui = render(<GoalDock state={{ goal: current }} observation={{}} disabled={false} mutate={mutate} />);
    fireEvent.click(ui.getByRole('button', { name: 'Edit goal objective' }));
    fireEvent.change(ui.getByRole('textbox'), { target: { value: 'Draft' } });
    // Same objective and budget, one more consumed round and a new revision:
    // the ordinary shape of an admitted Goal continuation.
    ui.rerender(<GoalDock state={{ goal: goal({ reference: { id: 'goal-1', revision: '4' }, autonomous_rounds_consumed: 1 }) }} observation={{}} disabled={false} mutate={mutate} />);
    expect(ui.getByRole('textbox')).toHaveProperty('value', 'Draft');
    fireEvent.keyDown(ui.getByRole('textbox'), { key: 'Escape' });
    expect(ui.container.textContent).toContain('Active Goal');
    expect(ui.container.textContent).not.toContain('r4');
    expect(ui.getByRole('button', { name: 'Pause goal' })).toBeTruthy();
    expect(ui.queryByRole('button', { name: 'Resume goal' })).toBeNull();
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
    expect(dock('Goal').textContent).toContain('Active Goal');
    expect(goalControls()).toHaveLength(1);
    expect(server.client.getSnapshot().uncertain.map(item => item.method)).toEqual(['goal/control']);
  });
  it('a control whose attachment changed settles as obsolete without touching the new view', async () => {
    server.snapshots.set('A', withGoal(goal()));
    await server.attached('A');
    server.held.add('goal/control');
    const work = server.client.controlGoal('A', goal().reference, { action: 'pause' });
    const request = await server.waitFor('goal/control', 1);
    await server.client.release('A'); await server.client.attach('A');
    server.reply(request);
    expect(await work).toEqual({ status: 'obsolete' });
    expect(server.client.getSnapshot().views.A).toMatchObject({ attachment: 'attached', error: undefined });
    const mutate = vi.fn(async (): Promise<GoalControlOutcome> => ({ status: 'obsolete' }));
    const ui = render(<GoalDock state={{ goal: goal() }} observation={{}} disabled={false} mutate={mutate} />);
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
    // Only human authoritative rows expose native Edit/Remove; no per-row Steer.
    expect(within(dock('Queue')).getAllByRole('button')).toHaveLength(3);
    expect(Object.getOwnPropertyNames(AppServerClient.prototype).filter(name => /queue|inbox/i.test(name))).toEqual([]);
    await update(running(withQueue([inbound('8', 'Continue')])));
    expect(within(dock('Queue')).queryByRole('button', { expanded: false })).toBeNull();
    expect(dock('Queue').textContent).toContain('Continue');
    expect(server.client.getSnapshot().views.A.snapshot?.inbound.pending?.[0].sequence).toBe('8');
    await update(running());
    expect(region('Queue')).toBeNull();
    expect(JSON.stringify(localStorage)).not.toContain('First queued');
    expect(JSON.stringify(server.client.getSnapshot().views.A.settings)).not.toContain('First queued');
  });
  it('composer delivery follows the authoritative attempt and labels the shared inbound semantics', async () => {
    await mount(snapshot());
    expect(screen.getByRole('button', { name: 'Send' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Steer' })).toBeNull();
    await update(running());
    expect(screen.getByRole('button', { name: 'Stop' })).toBeTruthy();
    expect(screen.queryByText('Queue and Steer enter the native mailbox at a safe boundary')).toBeNull();
    expect(screen.queryByLabelText('Delivery')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Send' })).toBeNull();
    await sendQueued('While running');
    await server.waitFor('turn/start', 1);
    expect(methods()).not.toContain('turn/steer');
    fireEvent.change(screen.getByLabelText('Message'), { target: { value: 'Steer running attempt' } });
    expect(screen.getByRole('button', { name: 'Queue' })).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Stop' })).toBeNull();
    await act(async () => fireEvent.keyDown(screen.getByLabelText('Message'), { key: 'Enter', ctrlKey: true }));
    const steered = await server.waitFor('turn/steer', 1);
    expect(steered.params).toMatchObject({ target: server.target('A'), content: [{ type: 'text', text: 'Steer running attempt' }] });
    await update(snapshot());
    expect(screen.getByRole('button', { name: 'Send' })).toBeTruthy();
    expect(screen.queryByLabelText('Delivery')).toBeNull();
    fireEvent.change(screen.getByLabelText('Message'), { target: { value: 'Idle again' } });
    await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Send' })));
    expect((await server.waitFor('turn/start', 2)).params).toMatchObject({ content: [{ type: 'text', text: 'Idle again' }] });
  });
  it('an unacknowledged request is composer transport state, never a queue row', async () => {
    await mount(running(withQueue([inbound('1', 'Native row')])));
    server.held.add('turn/start');
    await sendQueued('Queued draft');
    const turn = await server.waitFor('turn/start', 1);
    expect(screen.getByRole('button', { name: 'Send' })).toHaveProperty('disabled', true);
    expect(screen.getByText('Awaiting acknowledgement…')).toBeTruthy();
    // No count header: the in-flight request is not counted as queued.
    expect(within(dock('Queue')).queryByRole('button', { expanded: false })).toBeNull();
    expect(dock('Queue').querySelectorAll('li')).toHaveLength(1);
    expect(dock('Queue').querySelector('[data-submission-echo]')).toBeNull();
    expect(dock('Queue').textContent).not.toContain('Queued draft');
    expect(server.client.getSnapshot().views.A.submissions ?? []).toEqual([]);
    await act(async () => { server.reply(turn); });
    // Acceptance names the server MessageId; only now may provisional presentation exist.
    const header = await within(dock('Queue')).findByRole('button', { expanded: false });
    expect(header.textContent).toContain('2 queued');
    expect(within(header).getByRole('status').textContent).toBe('1 queued · updating…');
    fireEvent.click(header);
    const echo = dock('Queue').querySelector('[data-submission-echo]')!;
    expect(echo.getAttribute('data-accepted-message-id')).toBe('accepted-user');
    expect(echo.textContent).toContain('Queued · updating…');
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
    expect(screen.getByLabelText('Message')).toHaveProperty('value', 'Lost acknowledgement');
    expect(region('Queue')).toBeNull();
    expect(server.client.getSnapshot().uncertain.map(item => item.method)).toEqual(['turn/start']);
    expect(screen.getByLabelText('Session status').textContent).toContain('Needs verification');
    server.held.delete('turn/start');
    // The runtime did commit it; only an authoritative read may say so.
    server.snapshots.set('A', running(withQueue([inbound('5', 'Lost acknowledgement', { id: 'accepted-user' })])));
    await act(() => server.connect());
    await waitFor(() => expect(dock('Queue').querySelector('[data-message-id="accepted-user"]')).toBeTruthy());
    expect(dock('Queue').querySelector('[data-submission-echo]')).toBeNull();
    expect(methods().filter(method => method === 'turn/start')).toHaveLength(1);
    expect(screen.getByLabelText('Message')).toHaveProperty('value', 'Lost acknowledgement');
  });
});

describe('Composer context stack lifecycle', () => {
  it('orders Todo, Goal, Queue, Composer and each card appears independently without state leakage', async () => {
    const full = running(withQueue([inbound('1', 'one'), inbound('2', 'two')], withGoal(goal(), withTodos([task('1', 'pending')]))));
    const ui = await mount(full);
    const order = () => [...ui.container.querySelector('[data-composer-context-stack]')!.children].map(node => node.getAttribute('aria-label') ?? (node.querySelector('[data-composer-card]') ? 'Composer' : 'unknown'));
    expect(order()).toEqual(['To-dos', 'Goal', 'Queue', 'Composer']);
    const message = screen.getByLabelText('Message');
    fireEvent.change(message, { target: { value: 'Independent composer draft' } });
    fireEvent.click(within(dock('To-dos')).getByRole('button', { expanded: false }));
    fireEvent.click(goalButton('Edit goal objective'));
    fireEvent.change(within(dock('Goal')).getByRole('textbox'), { target: { value: 'Kept draft' } });
    // Queue disappears: Todo disclosure and Goal draft are unaffected.
    await update(running(withGoal(goal(), withTodos([task('1', 'pending')]))));
    expect(order()).toEqual(['To-dos', 'Goal', 'Composer']);
    expect(screen.getByLabelText('Message')).toBe(message);
    expect(message).toHaveProperty('value', 'Independent composer draft');
    expect(within(dock('To-dos')).getByRole('button', { expanded: true })).toBeTruthy();
    expect(within(dock('Goal')).getByRole('textbox')).toHaveProperty('value', 'Kept draft');
    // Todo disappears and Queue returns collapsed; Goal keeps its own draft.
    await update(running(withQueue([inbound('3', 'three'), inbound('4', 'four')], withGoal(goal()))));
    expect(order()).toEqual(['Goal', 'Queue', 'Composer']);
    expect(screen.getByLabelText('Message')).toBe(message);
    expect(message).toHaveProperty('value', 'Independent composer draft');
    expect(within(dock('Queue')).getByRole('button', { expanded: false })).toBeTruthy();
    expect(within(dock('Goal')).getByRole('textbox')).toHaveProperty('value', 'Kept draft');
    await update(withTodos([task('1', 'pending')]));
    expect(order()).toEqual(['To-dos', 'Composer']);
    expect(screen.getByLabelText('Message')).toBe(message);
    expect(message).toHaveProperty('value', 'Independent composer draft');
    expect(within(dock('To-dos')).getByRole('button', { expanded: false })).toBeTruthy();
  });
  it('R08: the stack composes fixed seats, and an empty or absent Todo occupies none of them', () => {
    const stack = (state: TodoDockState) => render(<ComposerContextStack todo={<TodoDock state={state} />} goal={null}
      queue={<QueueDock rows={[inbound('1', 'row')]} submissions={[]} running={false} />} composer={<div data-composer-card />} />);
    const seats = (ui: ReturnType<typeof stack>) => [...ui.container.firstElementChild!.children].map(node => node.getAttribute('aria-label') ?? 'Composer');
    expect(seats(stack({ kind: 'current', tasks: [task('1', 'pending')] }))).toEqual(['To-dos', 'Queue', 'Composer']);
    // Composed-empty, deleted-only and absent all leave the composed layout with no
    // Todo seat at all: no wrapper, separator or reserved stack height to lay out.
    for (const state of [todoDock(withTodos([])), todoDock(withTodos([task('1', 'deleted')])), todoDock(snapshot())]) {
      cleanup();
      const empty = stack(state);
      expect(seats(empty)).toEqual(['Queue', 'Composer']);
      expect(empty.container.querySelector('[data-todo-state]')).toBeNull();
      expect(empty.container.textContent).not.toContain('No current tasks');
    }
  });
  it('R08: the composed composer stack reserves no Todo seat while the current list is empty', async () => {
    const ui = await mount(withTodos([task('1', 'pending')]));
    const seats = () => [...ui.container.querySelector('[data-composer-context-stack]')!.children].map(node => node.getAttribute('aria-label') ?? 'Composer');
    expect(seats()).toEqual(['To-dos', 'Composer']);
    await update(withTodos([]));
    // Only the composer card remains; the stack gap has nothing left to separate.
    expect(seats()).toEqual(['Composer']);
    expect(ui.container.querySelector('[data-todo-state]')).toBeNull();
  });
  it('Session views never share dock presentation state', async () => {
    server.snapshots.set('B', withTodos([task('1', 'pending')], snapshot('B')));
    await mount(withTodos([task('1', 'pending')]), 'B');
    fireEvent.click(within(dock('To-dos')).getByRole('button', { expanded: false }));
    fireEvent.change(screen.getByLabelText('Message'), { target: { value: 'Only Session A' } });
    fireEvent.click(screen.getByRole('button', { name: 'Open Session B' }));
    expect(screen.getByLabelText('Message')).toHaveProperty('value', '');
    expect(within(dock('To-dos')).getByRole('button', { expanded: false })).toBeTruthy();
  });
  it('disconnect clears only accepted presentation echoes; reconnect rebuilds docks from the new authoritative snapshot', async () => {
    await mount(running(withQueue([inbound('1', 'Pending before loss')], withGoal(goal(), withTodos([task('1', 'in_progress')])))));
    await sendQueued('Echo only');
    const header = await within(dock('Queue')).findByRole('button', { expanded: false });
    fireEvent.click(header);
    expect(dock('Queue').querySelectorAll('[data-submission-echo]')).toHaveLength(1);
    const before = methods().length;
    act(() => server.socket.close());
    // Last observations stay visible but inert; nothing was cancelled or settled.
    expect(dock('Queue').querySelectorAll('[data-submission-echo]')).toHaveLength(0);
    expect(dock('Queue').textContent).toContain('Pending before loss');
    expect(dock('To-dos').textContent).toContain('1 in progress');
    expect(goalButton('Pause goal')).toHaveProperty('disabled', true);
    expect(methods().slice(before)).toEqual([]);
    // Native state moved on while this browser was away.
    server.snapshots.set('A', withGoal(goal({ phase: 'paused', reference: { id: 'goal-1', revision: '9' } }), withTodos([task('1', 'completed')])));
    await act(() => server.connect());
    await waitFor(() => expect(dock('Goal').textContent).toContain('Paused Goal'));
    expect(server.client.getSnapshot().views.A.snapshot?.goal?.current?.reference.revision).toBe('9');
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

describe('exact pending QueueDock mutations', () => {
  const row = () => inbound('7', 'before');
  it('edits an authoritative occurrence with its original revision and waits for readback', async () => {
    let finish!: (value: import('../src/client/app-server').InboundControlOutcome) => void;
    const edit = vi.fn(() => new Promise<import('../src/client/app-server').InboundControlOutcome>(resolve => { finish = resolve; }));
    const ui = render(<QueueDock rows={[row()]} submissions={[]} running edit={edit} remove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Edit queued message' }), { target: { value: 'after' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(edit).toHaveBeenCalledWith({ sequence: '7', message_id: 'message-7', revision: '0' }, 'after');
    expect(screen.getByRole('button', { name: 'Remove' })).toHaveProperty('disabled', true);
    await act(async () => finish({ status: 'known', outcome: { status: 'applied' }, observed: false }));
    expect(screen.getByText('before')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true);
    ui.rerender(<QueueDock rows={[{ ...row(), revision: '1', message: { ...row().message, content: [{ type: 'text', text: 'after' }] } }]} observation={snapshot()} submissions={[]} running edit={edit} remove={vi.fn()} />);
    expect(screen.getByText('after')).toBeTruthy();
    expect(edit).toHaveBeenCalledTimes(1);
  });
  it('preserves stale drafts and requires deliberate reconciliation', async () => {
    const edit = vi.fn(async () => ({ status: 'known' as const, outcome: { status: 'conflict' as const }, observed: true }));
    const ui = render(<QueueDock rows={[row()]} submissions={[]} running edit={edit} />);
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Edit queued message' }), { target: { value: 'my draft' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    await screen.findByText(/This item changed/);
    ui.rerender(<QueueDock rows={[{ ...row(), revision: '1' }]} submissions={[]} running edit={edit} />);
    expect(screen.getByRole('textbox')).toHaveProperty('value', 'my draft');
    expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true);
    fireEvent.click(screen.getByRole('button', { name: 'Use latest version' }));
    expect(edit).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', false);
    ui.rerender(<QueueDock rows={[]} submissions={[]} running edit={edit} />);
    expect(screen.getByRole('textbox')).toHaveProperty('value', 'my draft');
    expect(screen.queryByRole('button', { name: 'Remove' })).toBeNull();
  });
  it('keeps an uncertain remove locked without inventing or replaying a snapshot', async () => {
    const remove = vi.fn(async () => ({ status: 'uncertain' as const }));
    const ui = render(<QueueDock rows={[row()]} submissions={[]} running remove={remove} />);
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
    await screen.findByText(/Outcome uncertain/);
    expect(screen.getByText('before')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Remove' })).toHaveProperty('disabled', true);
    ui.rerender(<QueueDock rows={[]} observation={snapshot()} submissions={[]} running remove={remove} />);
    expect(screen.queryByText('before')).toBeNull();
    expect(remove).toHaveBeenCalledTimes(1);
  });
  it('omits provisional controls and disables unsafe typed-content editing', () => {
    const mixed = { ...row(), message: { ...row().message, content: [{ type: 'text' as const, text: 'one' }, { type: 'uploaded_file' as const, batch_id: 'owned-batch', name: 'report.pdf' }] } };
    const ui = render(<QueueDock rows={[]} submissions={[{ messageId: 'echo', content: [{ type: 'text', text: 'provisional' }] }]} running edit={vi.fn()} remove={vi.fn()} />);
    expect(screen.queryByRole('button', { name: 'Edit' })).toBeNull();
    ui.rerender(<QueueDock rows={[mixed]} submissions={[]} running edit={vi.fn()} remove={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'Edit' })).toHaveProperty('disabled', true);
    expect(screen.getByRole('button', { name: 'Remove' })).toHaveProperty('disabled', false);
  });
});
