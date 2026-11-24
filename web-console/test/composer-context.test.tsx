import {readCut} from './fixture';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RuntimeClientGoalSnapshot, RuntimeClientSnapshot, TodoTask } from '../../protocol/app-server/v81';
import { App } from '../src/app/App';
import { ComposerContextStack } from '../src/app/composer/ComposerContextStack';
import { GoalDock } from '../src/app/composer/GoalDock';
import { QueueDock } from '../src/app/composer/QueueDock';
import { TodoDock } from '../src/app/composer/TodoDock';
import { goalDock, queueRows, todoDock, type TodoDockState } from '../src/bindings/composer-context';
import { AppServerClient } from '../src/client/app-server';
import type { GoalControlOutcome, InboundControlOutcome } from '../src/client/contracts';
import { ArtifactContext } from '../src/app/components/Artifact';
import { ArtifactResources } from '../src/client/artifacts';
import { Server, snapshot } from './fixture';

const task = (id: string, status: TodoTask['status'], extra: Partial<TodoTask> = {}): TodoTask => ({ id, subject: `Task ${id}`, status, ...extra });
const goal = (extra: Partial<RuntimeClientGoalSnapshot> = {}): RuntimeClientGoalSnapshot => ({
  objective_deferred: false, blocked_reason_deferred: false, last_round_message_id_deferred: false, fields: [],
  reference: { id: 'goal-1', revision: '3' }, objective: 'Ship the docks', phase: 'active',
  autonomous_round_budget: 4, autonomous_rounds_consumed: 1, origin: { kind: 'runtime_control' }, ...extra,
});
const withTodos = (tasks: TodoTask[] | undefined, base = snapshot()): RuntimeClientSnapshot => ({ ...base, todos: tasks && { tasks, next_id: String(tasks.length + 1) } });
const withGoal = (current: RuntimeClientGoalSnapshot | null, base = snapshot()): RuntimeClientSnapshot => ({ ...base, goal: { current } });
const running = (base = snapshot()): RuntimeClientSnapshot => ({ ...base, attempt: { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 1 } });
const inbound = (sequence: string, text: string, message: { id?: string; source?: 'human' | 'runtime'; kind?: { goal_continuation: { id: string; revision: string } } } = {}) =>
  ({ revision: "0", sequence, message: { id: `message-${sequence}`, source: 'human' as const, kind: 'queued_message' as const, content: [{ type: 'text' as const, text }], ...message } });
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
  const call = { role: 'assistant' as const, id: 'history-call', content: [{ type: 'tool_call' as const, id: 'call-todo', tool_id: 'tool-todo', name: 'todo', arguments: { arguments: { action: 'create', subject: 'Historical task' } } }] };
  const result = { role: 'tool' as const, occurrence: { assistant_message_id: 'history-call', block_index: 0 }, id: 'history-result', tool_call_id: 'call-todo', tool_id: 'tool-todo', result: { status: { type: 'success' as const }, duration_ms: '1' } };
  return { ...base, transcript: { entries: [call, result].map((message, index) => ({ cursor: String(index + 1), item: { type: 'message' as const, message }, tool_calls: message.role === 'assistant' ? [{ message_id: call.id, occurrence: { locator: { type: 'by_message_id' as const, message_id: call.id }, block_index: 0 }, block_index: 0, call_id: 'call-todo', tool_id: 'tool-todo', name: 'todo', assembly_complete: true, committed_result_deferred: false, committed_result: result.result, state: { type: 'assembled' as const, arguments: JSON.stringify(call.content[0].arguments) } }] : [] })) } };
};

let server: Server;
beforeEach(() => { localStorage.clear(); server = new Server(); });
afterEach(() => { cleanup(); server.client.disconnect(); });
async function mount(initial: RuntimeClientSnapshot, ...others: string[]) {
  server.snapshots.set('A', initial);
  await server.attached('A', ...others);
  localStorage.setItem('rustx-console-view-v3', JSON.stringify({ endpoint: 'ws://127.0.0.1:8080/', openViews: ['A', ...others], route: { kind: 'session', sessionId: 'A' } }));
  return render(<App client={server.client} workspaceHost={server.workspaceHost} />);
}
const region = (name: 'To-dos' | 'Goal' | 'Queue') => screen.queryByRole('region', { name });
const dock = (name: 'To-dos' | 'Goal' | 'Queue') => screen.getByRole('region', { name });
const methods = () => server.requests.map(item => item.request.method);
const goalControls = () => server.requests.flatMap(item => item.request.method === 'goal/control' ? [item.request.params.control] : []);
const snapshotReads = () => methods().filter(method => method === 'session/checkpoint').length;
const goalButton = (name: string) => within(dock('Goal')).getByRole('button', { name });
const CONFIGURATION_WRITES = ['settings/replace', 'configuration/sourceWrite', 'configuration/reconcile', 'session/setModel', 'thread/start'];
const update = (next: RuntimeClientSnapshot) => act(() => server.update('A', next));
/** Answer a held authoritative read with a transport-level failure. */
const failRead = (request: { id: string | number }) => act(async () => {
  server.socket.deliver({ jsonrpc: '2.0', id: request.id, error: { code: -32000, message: 'Snapshot unavailable' } });
});
/** A later native event whose successful snapshot read is the next authoritative observation. */
const recoverAuthority = async () => { server.held.delete('session/checkpoint'); await act(async () => server.client.refresh('A')); };
const sendQueued = async (text: string) => {
  fireEvent.change(screen.getByLabelText('Message'), { target: { value: text } });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Queue' })); });
};

describe('Todo dock binds only the composed native Todo projection', () => {
  it('T15: an unloaded native task head stays visible and distinct from an empty list', () => {
    const state = todoDock(withTodos([]), { domain: 'todos.tasks', revision: '7', total: '1000', loaded: '0', next_offset: '0' });
    expect(state).toEqual({ kind: 'current', tasks: [], deferred: true });
    render(<TodoDock state={state}/>);
    expect(dock('To-dos').textContent).toContain('Loading…');
    fireEvent.click(within(dock('To-dos')).getByRole('button'));
    expect(within(dock('To-dos')).getByRole('status').textContent).toBe('Loading…');
    expect(within(dock('To-dos')).queryAllByRole('listitem')).toHaveLength(0);
    expect(todoDock(snapshot(), { domain: 'todos.tasks', revision: '7', total: '1000', loaded: '0', next_offset: '0' })).toEqual({ kind: 'absent' });
  });
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
    for (const [index, label, dot] of [[0, 'Completed', 'done'], [1, 'In progress', 'ongoing'], [2, 'Pending', 'idle']] as const) {
      expect(within(rows[index]).getByRole('img', { name: label }).firstElementChild?.getAttribute('data-state')).toBe(dot);
    }
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
    expect(document.querySelector('[data-tool-call-id="call-todo"]')).not.toBeNull();
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
    expect(document.querySelector('[data-tool-call-id="call-todo"]')).not.toBeNull();
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
    server.held.add('session/checkpoint');
    fireEvent.click(goalButton('Pause goal'));
    await failRead(await server.waitFor('session/checkpoint', reads + 1));
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
    server.held.add('session/checkpoint');
    fireEvent.click(goalButton('Pause goal'));
    await failRead(await server.waitFor('session/checkpoint', reads + 1));
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
    const outcomes: GoalControlOutcome[] = [{ status: 'applied', observed: false}, { status: 'rejected', reason: 'Refused by GoalDomain', observed: false }];
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
    const mutate = vi.fn(async (): Promise<GoalControlOutcome> => ({ status: 'applied', observed: true}));
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
    const mutate = vi.fn(async (): Promise<GoalControlOutcome> => ({ status: 'applied', observed: true}));
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
    await act(()=>server.connect());
    expect(within(dock('Goal')).getByRole('status').textContent).toContain('uncertain');
    await act(()=>server.client.refresh('A'));
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
    // Only human authoritative rows expose native Edit/Remove/Steer (header + 3 row actions).
    expect(within(dock('Queue')).getAllByRole('button')).toHaveLength(4);
    expect(within(rows[0]).getByRole('button', { name: 'Edit' })).toBeTruthy();
    expect(within(rows[0]).getByRole('button', { name: 'Remove' })).toBeTruthy();
    expect(within(rows[0]).getByRole('button', { name: 'Steer queued message' })).toBeTruthy();
    expect(within(rows[1]).queryAllByRole('button')).toHaveLength(0);
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
    await server.waitFor('inbound/queue', 1);
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
    expect((await server.waitFor('inbound/queue', 2)).params).toMatchObject({ content: [{ type: 'text', text: 'Idle again' }] });
  });
  it('an in-flight queue request renders a Sending row with disabled actions and hands off to accepted then native rows', async () => {
    await mount(running(withQueue([inbound('1', 'Native row')])));
    server.held.add('inbound/queue');
    await sendQueued('Queued draft');
    const turn = await server.waitFor('inbound/queue', 1);
    expect(screen.getByRole('button', { name: 'Sending…' })).toHaveProperty('disabled', true);
    expect(screen.queryByText('Awaiting acknowledgement…')).toBeNull();
    expect(screen.getByRole('textbox', { name: 'Message' }).closest('[data-composer-card]')?.getAttribute('aria-busy')).toBe('true');
    // Collapsed header exposes the Sending status while a pre-ack row is in flight.
    const sendingHeader = within(dock('Queue')).getByRole('button', { expanded: false });
    expect(sendingHeader.textContent).toContain('2 queued');
    expect(within(sendingHeader).getByRole('status').textContent).toBe('Sending…');
    fireEvent.click(sendingHeader);
    const sendingEcho = dock('Queue').querySelector('[data-sending-input-id]')!;
    expect(sendingEcho).toBeTruthy();
    expect(sendingEcho.textContent).toContain('Queued draft');
    expect(within(sendingEcho as HTMLElement).getByRole('status').textContent).toBe('Sending…');
    for (const name of ['Edit', 'Remove', 'Steer queued message']) {
      expect(within(sendingEcho as HTMLElement).getByRole('button', { name })).toHaveProperty('disabled', true);
    }
    expect(server.client.getSnapshot().views.A.submissions ?? []).toEqual([]);
    await act(async () => { server.reply(turn); });
    // Acceptance names the server MessageId; the sending row hands off to the accepted provisional row.
    await waitFor(() => expect(dock('Queue').querySelector('[data-sending-input-id]')).toBeNull());
    const echo = dock('Queue').querySelector('[data-accepted-message-id="accepted-user"]')!;
    expect(echo).toBeTruthy();
    expect(echo.textContent).toContain('Sending…');
    expect(server.client.getSnapshot().views.A.submissions?.map(item => item.messageId)).toEqual(['accepted-user']);
  });
  it('keeps an acknowledged steer out of the chat until the provider-adopted row arrives', async () => {
    const state = running(withQueue([inbound('1', 'Next turn')]));
    state.attempt!.in_flight = { message_id: 'reply', blocks: [{ type: 'text', block_index: 0, text: 'Still answering' }] };
    const ui = await mount(state);
    await act(async () => { await server.client.send('A', 'Change direction', [], 'steer'); });
    expect(ui.container.querySelector('[data-provisional-message-id]')).toBeNull();
    expect(ui.container.querySelector('[data-chat-flow-kind="user"]')).toBeNull();
    expect(screen.queryByText('Change direction')).toBeNull();
    expect(dock('Queue').textContent).not.toContain('Change direction');
    const message = { ...inbound('2', 'Change direction', { id: 'accepted-user' }).message, role: 'user' as const };
    await update({ ...state, messages: [message], transcript: { entries: [{ cursor: '2', item: { type: 'message', message } }] } });
    expect(screen.getAllByText('Change direction')).toHaveLength(1);
    expect(ui.container.querySelector('[data-provisional-message-id]')).toBeNull();
    expect(ui.container.querySelector('[data-chat-flow-kind="user"]')?.textContent).toContain('Change direction');
  });
  it('does not paint a direct steer at enqueue or mailbox admission', async () => {
    const state = running();
    const ui = await mount(state);
    server.held.add('turn/steer');
    let sending!: Promise<unknown>;
    await act(async () => { sending = server.client.send('A', 'Mid-turn note', [], 'steer'); });
    const request = await server.waitFor('turn/steer', 1);
    if (request.method !== 'turn/steer') throw Error('Expected steer');
    expect(ui.container.querySelector('[data-pending-steering]')).toBeNull();
    expect(screen.queryByText('Mid-turn note')).toBeNull();
    await update({ ...state, inbound: { pending: [{ revision: '0', sequence: '1', input_id: request.params.inputId,
      message: { id: 'accepted-user', source: 'human', kind: 'message', content: [{ type: 'text', text: 'Mid-turn note' }] } }] },
      transcript: { entries: [{ cursor: '4', item: { type: 'message', message: { role: 'user', id: 'accepted-user', source: 'human', kind: 'message', content: [{ type: 'text', text: 'Mid-turn note' }] } } }] } });
    expect(screen.queryByText('Mid-turn note')).toBeNull();
    expect(region('Queue')).toBeNull();
    await act(async () => { server.reply(request); await sending; });
    expect(screen.queryByText('Mid-turn note')).toBeNull();
    const message = { role: 'user' as const, id: 'accepted-user', source: 'human' as const, kind: 'message' as const, content: [{ type: 'text' as const, text: 'Mid-turn note' }] };
    await update({ ...state, inbound: { pending: [] }, messages: [message], transcript: { entries: [{ cursor: '4', item: { type: 'message', message } }] } });
    expect(screen.getAllByText('Mid-turn note')).toHaveLength(1);
  });
  it('places an adopted steer after job activity committed while it was waiting', async () => {
    const steer = 'Steer while answering';
    const notice = 'Background execution job-one (bash) settled: outcome_unknown\n\nResult:\nExternal outcome was not proven';
    const state = running();
    state.attempt!.in_flight = { message_id: 'reply', blocks: [{ type: 'text', block_index: 0, text: 'Still answering' }] };
    state.inbound = { pending: [{ revision: '0', sequence: '2', input_id: 'steer-input', message: { id: 'steer-1', source: 'human', kind: 'message', content: [{ type: 'text', text: steer }] } }] };
    state.transcript = { entries: [
      { cursor: '3', item: { type: 'message', message: { role: 'user', id: 'background-job-one-terminal', source: 'runtime', kind: 'message', content: [{ type: 'text', text: notice }] } } },
    ] };
    const ui = await mount(state);
    const background = ui.container.querySelector('[data-inbound-source="job"]');
    expect(background).toBeTruthy();
    expect(screen.queryByText(steer)).toBeNull();
    expect(ui.container.querySelector('[data-pending-steering]')).toBeNull();
    const status = ui.container.querySelector('[data-chat-running]')!;
    expect(background!.compareDocumentPosition(status) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    await update({ ...state, inbound: { pending: [] }, transcript: { entries: [...state.transcript.entries!, { cursor: '4', item: { type: 'message', message: { role: 'user', id: 'steer-1', source: 'human', kind: 'message', content: [{ type: 'text', text: steer }] } } }] } });
    expect(background!.compareDocumentPosition(screen.getByText(steer)) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
  it('keeps a reserved queue seat in the dock until adoption, not as a chat bubble', async () => {
    const text = 'Queued until the next request';
    const row = inbound('4', text, { id: 'queued-seat' });
    const message = { ...row.message, role: 'user' as const };
    const state = running();
    state.inbound = { pending: [row] };
    state.transcript = { entries: [{ cursor: '8', item: { type: 'message', message } }] };
    const ui = await mount(state);
    expect(screen.getByLabelText('Canonical conversation').textContent).not.toContain(text);
    expect(dock('Queue').textContent).toContain(text);
    await update({ ...state, inbound: { pending: [] }, messages: [message] });
    expect(ui.container.querySelector('[data-chat-flow-kind="user"]')?.textContent).toContain(text);
    expect(screen.queryByRole('region', { name: 'Queue' })).toBeNull();
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
    server.held.add('inbound/queue');
    await sendQueued('Lost acknowledgement');
    await server.waitFor('inbound/queue', 1);
    act(() => server.socket.close());
    expect(screen.getByLabelText('Message')).toHaveProperty('value', 'Lost acknowledgement');
    expect(region('Queue')).toBeNull();
    expect(server.client.getSnapshot().uncertain.map(item => item.method)).toEqual(['inbound/queue']);
    expect(screen.getByLabelText('Session status').textContent).toContain('Needs verification');
    server.held.delete('inbound/queue');
    // The runtime did commit it; only an authoritative read may say so.
    server.snapshots.set('A', running(withQueue([inbound('5', 'Lost acknowledgement', { id: 'accepted-user' })])));
    await act(() => server.connect());
    await waitFor(() => expect(dock('Queue').querySelector('[data-message-id="accepted-user"]')).toBeTruthy());
    expect(dock('Queue').querySelector('[data-submission-echo]')).toBeNull();
    expect(methods().filter(method => method === 'inbound/queue')).toHaveLength(1);
    expect(screen.getByLabelText('Message')).toHaveProperty('value', 'Lost acknowledgement');
  });
});

describe('Composer context stack lifecycle', () => {
  it('orders Todo, Goal, Queue, Composer and each card appears independently without state leakage', async () => {
    const full = running(withQueue([inbound('1', 'one'), inbound('2', 'two')], withGoal(goal(), withTodos([task('1', 'pending')]))));
    const ui = await mount(full);
    const order = () => [...ui.container.querySelector('[data-composer-context-stack]')!.children].map(node => (node.hasAttribute('data-context-seat') ? 'Context' : node.getAttribute('aria-label')) ?? (node.querySelector('[data-composer-card]') ? 'Composer' : 'unknown'));
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
    const seats = (ui: ReturnType<typeof stack>) => [...ui.container.firstElementChild!.children].map(node => (node.hasAttribute('data-context-seat') ? 'Context' : node.getAttribute('aria-label')) ?? 'Composer');
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
    const seats = () => [...ui.container.querySelector('[data-composer-context-stack]')!.children].map(node => (node.hasAttribute('data-context-seat') ? 'Context' : node.getAttribute('aria-label')) ?? 'Composer');
    expect(seats()).toEqual(['To-dos', 'Composer']);
    await update(withTodos([]));
    // Only the Composer remains: an idle Context seat and an empty Todo reserve no space.
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
    expect(methods().filter(method => ['goal/control', 'turn/interrupt', 'turn/steer'].includes(method))).toEqual([]);
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
  it('T11/T15 a deferred pending preview cannot replace original text, while removal remains exact', async () => {
    const edit=vi.fn(),remove=vi.fn(async()=>({status:'known' as const,outcome:{status:'applied' as const,read_cut:readCut()},observed:true}));
    render(<QueueDock rows={[{...inbound('7','partial preview'),deferred:true}]} submissions={[]} running edit={edit} remove={remove}/>);
    expect(screen.getByRole('button',{name:'Edit'})).toHaveProperty('disabled',true);
    fireEvent.click(screen.getByRole('button',{name:'Edit'})); expect(edit).not.toHaveBeenCalled();
    await act(async()=>fireEvent.click(screen.getByRole('button',{name:'Remove'})));
    expect(remove).toHaveBeenCalledWith({sequence:'7',revision:'0'});
  });

  const row = () => {const value=inbound('7','before');return {...value,message:{...value.message,kind:'queued_message' as const}};};
  it('edits an authoritative occurrence with its original revision and waits for readback', async () => {
    let finish!: (value: import('../src/client/contracts').InboundControlOutcome) => void;
    const edit = vi.fn(() => new Promise<import('../src/client/contracts').InboundControlOutcome>(resolve => { finish = resolve; }));
    const ui = render(<QueueDock rows={[row()]} submissions={[]} running edit={edit} remove={vi.fn()} />);
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Edit queued message' }), { target: { value: 'after' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));
    expect(edit).toHaveBeenCalledWith({ sequence: '7', revision: '0' }, 'after');
    expect(screen.getByRole('region', { name: 'Queue' }).getAttribute('aria-busy')).toBe('true');
    expect(screen.queryByText('Updating queue…')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Remove' })).toBeNull();
    const receipt = { ...readCut(), mutation_revision: '1' };
    await act(async () => finish({ status: 'known', outcome: { status: 'applied', read_cut: receipt }, observed: false }));
    expect(screen.getByRole('textbox').closest('[data-inbound-sequence]')?.getAttribute('data-inbound-sequence')).toBe('7');
    expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true);
    ui.rerender(<QueueDock rows={[row()]} observation={snapshot()} readCut={readCut()} submissions={[]} running edit={edit} remove={vi.fn()} />);
    expect(screen.getByRole('textbox')).toHaveProperty('value', 'after');
    ui.rerender(<QueueDock rows={[{ ...row(), revision: '1', message: { ...row().message, content: [{ type: 'text', text: 'after' }] } }]} observation={snapshot()} readCut={receipt} submissions={[]} running edit={edit} remove={vi.fn()} />);
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(screen.getByText('after')).toBeTruthy();
    expect(edit).toHaveBeenCalledTimes(1);
  });
  it('preserves edits across revision conflicts and keeps unsaved text when adoption removes the last row', async () => {
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    let refuseClipboard = false;
    const writeText = vi.fn(async (_text: string) => {
      if (refuseClipboard) throw new Error('denied');
    });
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    try {
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
      // Native adoption removes the exact row as the last row: preserve the unsaved draft without auto-sending.
      ui.rerender(<QueueDock rows={[]} submissions={[]} running edit={edit} />);
      expect(ui.container.querySelector('[data-queue-dock]')).toBeTruthy();
      expect(ui.container.querySelector('[data-queue-draft="detached"]')).toBeTruthy();
      const editor = screen.getByRole('textbox', { name: 'Edit queued message' }) as HTMLTextAreaElement;
      expect(editor.value).toBe('my draft');
      expect(editor.disabled).toBe(false);
      expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true);
      expect(screen.queryByRole('button', { name: 'Use latest version' })).toBeNull();
      expect(screen.getByText('This message is no longer queued. Your draft is preserved.')).toBeTruthy();
      // Neither Enter nor Escape submits or clears the preserved detached draft.
      fireEvent.keyDown(editor, { key: 'Enter' });
      fireEvent.keyDown(editor, { key: 'Escape' });
      expect(edit).toHaveBeenCalledTimes(1);
      expect(screen.getByRole('textbox', { name: 'Edit queued message' })).toHaveProperty('value', 'my draft');
      // Copy works without clearing the draft.
      await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Copy draft' })); });
      expect(writeText).toHaveBeenCalledExactlyOnceWith('my draft');
      expect(screen.getByText('Draft copied.')).toBeTruthy();
      expect(screen.getByRole('textbox', { name: 'Edit queued message' })).toHaveProperty('value', 'my draft');
      // A refused clipboard write reports copy failure and keeps the draft intact.
      refuseClipboard = true;
      await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Copy draft' })); });
      expect(screen.getByText('Could not copy draft. Select the text and copy it manually.')).toBeTruthy();
      expect(screen.getByRole('textbox', { name: 'Edit queued message' })).toHaveProperty('value', 'my draft');
      // Only explicit discard clears the detached draft and hides the empty dock.
      fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
      expect(screen.queryByRole('textbox')).toBeNull();
      expect(ui.container.querySelector('[data-queue-dock]')).toBeNull();
      expect(edit).toHaveBeenCalledTimes(1);
    } finally {
      if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard);
      else Reflect.deleteProperty(navigator, 'clipboard');
    }
  });
  it('preserves unsaved text when native adoption removes the edited row while other rows remain queued', async () => {
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    try {
      const first = inbound('7', 'first queued');
      const second = inbound('8', 'second queued');
      const edit = vi.fn<() => Promise<InboundControlOutcome>>();
      const remove = vi.fn<() => Promise<InboundControlOutcome>>(async () => ({ status: 'known', outcome: { status: 'applied', read_cut: readCut() }, observed: true }));
      const ui = render(<QueueDock rows={[first, second]} submissions={[]} running edit={edit} remove={remove} />);
      fireEvent.click(screen.getByRole('button', { expanded: false }));
      const firstRow = ui.container.querySelector('[data-inbound-sequence="7"]') as HTMLElement;
      fireEvent.click(within(firstRow).getByRole('button', { name: 'Edit' }));
      fireEvent.change(screen.getByRole('textbox', { name: 'Edit queued message' }), { target: { value: 'edited first' } });
      // Native adoption removes row 7 while row 8 remains pending.
      ui.rerender(<QueueDock rows={[second]} submissions={[]} running edit={edit} remove={remove} />);
      expect(ui.container.querySelector('[data-inbound-sequence="7"]')).toBeNull();
      const remainingRow = ui.container.querySelector('[data-inbound-sequence="8"]') as HTMLElement;
      expect(within(remainingRow).getByText('second queued')).toBeTruthy();
      expect(within(remainingRow).getByRole('button', { name: 'Edit' })).toHaveProperty('disabled', true);
      expect(screen.getByRole('textbox', { name: 'Edit queued message' })).toHaveProperty('value', 'edited first');
      expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true);
      expect(screen.getByText('This message is no longer queued. Your draft is preserved.')).toBeTruthy();
      await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Copy draft' })); });
      expect(writeText).toHaveBeenCalledExactlyOnceWith('edited first');
      // Mutating another remaining row never clears the preserved detached draft.
      await act(async () => { fireEvent.click(within(remainingRow).getByRole('button', { name: 'Remove' })); });
      expect(remove).toHaveBeenCalledExactlyOnceWith({ sequence: '8', revision: '0' });
      expect(screen.getByRole('textbox', { name: 'Edit queued message' })).toHaveProperty('value', 'edited first');
      // Explicit discard clears only the draft and re-enables editing remaining rows.
      fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
      expect(screen.queryByRole('textbox')).toBeNull();
      expect(within(remainingRow).getByRole('button', { name: 'Edit' })).toHaveProperty('disabled', false);
      expect(edit).not.toHaveBeenCalled();
    } finally {
      if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard);
      else Reflect.deleteProperty(navigator, 'clipboard');
    }
  });
  it('preserves unsaved text when a pending edit returns not_pending, including across readback, until explicit discard', async () => {
    const originalClipboard = Object.getOwnPropertyDescriptor(navigator, 'clipboard');
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    try {
      let finish!: (value: InboundControlOutcome) => void;
      const edit = vi.fn(() => new Promise<InboundControlOutcome>(resolve => { finish = resolve; }));
      const ui = render(<QueueDock rows={[row()]} observation={snapshot()} submissions={[]} running edit={edit} />);
      fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
      fireEvent.change(screen.getByRole('textbox', { name: 'Edit queued message' }), { target: { value: 'pending edit draft' } });
      fireEvent.click(screen.getByRole('button', { name: 'Save' }));
      expect(edit).toHaveBeenCalledExactlyOnceWith({ sequence: '7', revision: '0' }, 'pending edit draft');
      // Even when not_pending settles with observed: false, Save stays disabled and the draft stays preserved.
      await act(async () => finish({ status: 'known', outcome: { status: 'not_pending' }, observed: false }));
      expect(screen.getByRole('textbox', { name: 'Edit queued message' })).toHaveProperty('value', 'pending edit draft');
      expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true);
      expect(screen.getAllByText('This message is no longer queued. Your draft is preserved.')).toHaveLength(1);
      // Authoritative readback removing the row still preserves the unsaved draft without auto-sending.
      ui.rerender(<QueueDock rows={[]} observation={snapshot()} submissions={[]} running edit={edit} />);
      expect(screen.getByRole('textbox', { name: 'Edit queued message' })).toHaveProperty('value', 'pending edit draft');
      expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true);
      expect(screen.getByText('This message is no longer queued. Your draft is preserved.')).toBeTruthy();
      await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Copy draft' })); });
      expect(writeText).toHaveBeenCalledExactlyOnceWith('pending edit draft');
      fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
      expect(screen.queryByRole('textbox')).toBeNull();
      expect(ui.container.querySelector('[data-queue-dock]')).toBeNull();
      expect(edit).toHaveBeenCalledTimes(1);

      // Also verify not_pending with observed: true keeps the draft preserved when the row leaves immediately.
      const editObserved = vi.fn<() => Promise<InboundControlOutcome>>(async () => ({ status: 'known', outcome: { status: 'not_pending' }, observed: true }));
      ui.rerender(<QueueDock rows={[row()]} observation={snapshot()} submissions={[]} running edit={editObserved} />);
      fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
      fireEvent.change(screen.getByRole('textbox', { name: 'Edit queued message' }), { target: { value: 'observed not_pending draft' } });
      await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Save' })); });
      ui.rerender(<QueueDock rows={[]} observation={snapshot()} submissions={[]} running edit={editObserved} />);
      expect(screen.getByRole('textbox', { name: 'Edit queued message' })).toHaveProperty('value', 'observed not_pending draft');
      expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true);
      expect(screen.getByText('This message is no longer queued. Your draft is preserved.')).toBeTruthy();
      fireEvent.click(screen.getByRole('button', { name: 'Discard draft' }));
      expect(ui.container.querySelector('[data-queue-dock]')).toBeNull();
      expect(editObserved).toHaveBeenCalledTimes(1);
    } finally {
      if (originalClipboard) Object.defineProperty(navigator, 'clipboard', originalClipboard);
      else Reflect.deleteProperty(navigator, 'clipboard');
    }
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
  it('keeps provisional controls disabled through acknowledgement and limits editing to text rows', () => {
    const mixed = { ...row(), message: { ...row().message, content: [{ type: 'text' as const, text: 'one' }, { type: 'uploaded_file' as const, batch_id: 'owned-batch', name: 'report.pdf' }] } };
    const ui = render(<QueueDock rows={[]} submissions={[{ messageId: 'echo', content: [{ type: 'text', text: 'provisional' }], placement: 'queued' }]} running edit={vi.fn()} remove={vi.fn()} steer={vi.fn()} />);
    for (const name of ['Edit', 'Remove', 'Steer queued message']) expect(screen.getByRole('button', { name })).toHaveProperty('disabled', true);
    ui.rerender(<QueueDock rows={[]} submissions={[{ messageId: 'sending:in-1', sendingInputId: 'in-1', content: [{ type: 'text', text: 'in flight' }], placement: 'queued' }]} running edit={vi.fn()} remove={vi.fn()} steer={vi.fn()} />);
    for (const name of ['Edit', 'Remove', 'Steer queued message']) {
      const btn = screen.getByRole('button', { name }) as HTMLButtonElement;
      expect(btn.disabled).toBe(true);
      expect(btn.title).toBe('Sending…');
    }
    ui.rerender(<QueueDock rows={[mixed]} submissions={[]} running edit={vi.fn()} remove={vi.fn()} steer={vi.fn()} />);
    expect(screen.getByRole('button', { name: 'Edit' })).toHaveProperty('disabled', true);
    expect(screen.getByRole('button', { name: 'Remove' })).toHaveProperty('disabled', false);
    expect(screen.getByRole('button', { name: 'Steer queued message' })).toHaveProperty('disabled', false);
    expect(screen.getByLabelText('Queued file report.pdf').textContent).toBe('report.pdf');
    expect(screen.getByLabelText('Queued file report.pdf').textContent).not.toMatch(/0B|undefined/);
  });

  it('steers a complete row only while running and reports steer failures', async () => {
    const mixed = { ...row(), message: { ...row().message, content: [{ type: 'uploaded_file' as const, batch_id: 'b', name: 'shot.png' }] } };
    const steer = vi.fn<() => Promise<InboundControlOutcome>>(async () => ({ status: 'known', outcome: { status: 'applied', read_cut: readCut() }, observed: true }));
    const ui = render(<QueueDock rows={[mixed]} submissions={[]} running steer={steer} />);
    const button = screen.getByRole('button', { name: 'Steer queued message' }) as HTMLButtonElement;
    expect(button.disabled).toBe(false);
    await act(async () => { fireEvent.click(button); });
    expect(steer).toHaveBeenCalledExactlyOnceWith({ sequence: '7', revision: '0' });

    ui.rerender(<QueueDock rows={[mixed]} submissions={[]} running={false} steer={steer} />);
    expect((screen.getByRole('button', { name: 'Steer queued message' }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByRole('button', { name: 'Steer queued message' }).getAttribute('title')).toBe('Steer is only available while running');

    steer.mockRejectedValueOnce(new Error('network lost'));
    ui.rerender(<QueueDock rows={[mixed]} submissions={[]} running steer={steer} />);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Steer queued message' })); });
    expect(screen.getByRole('status').textContent).toBe('Could not steer the queued message. Try again.');
    expect(screen.getByLabelText('Queued file shot.png')).toBeTruthy();
  });

  it('deduplicates a sending submission as soon as a native row matches its input_id', () => {
    const sending = { messageId: 'sending:input-42', sendingInputId: 'input-42', content: [{ type: 'text' as const, text: 'hello' }], placement: 'queued' as const };
    const ui = render(<QueueDock rows={[]} submissions={[sending]} running />);
    expect(ui.container.querySelector('[data-sending-input-id="input-42"]')).toBeTruthy();
    const nativeRow = { ...inbound('9', 'hello'), input_id: 'input-42' } as ReturnType<typeof inbound>;
    ui.rerender(<QueueDock rows={[nativeRow]} submissions={[sending]} running />);
    expect(ui.container.querySelector('[data-sending-input-id]')).toBeNull();
    expect(ui.container.querySelectorAll('li')).toHaveLength(1);
    expect(ui.container.querySelector('[data-inbound-sequence="9"]')).toBeTruthy();
  });

  it('keeps line breaks when editing a multiline queued message with Shift+Enter and Enter', async () => {
    const text = 'line one\n  line two\n\nline four';
    const edit = vi.fn(async (): Promise<InboundControlOutcome> => ({ status: 'known', outcome: { status: 'applied', read_cut: readCut() }, observed: true }));
    render(<QueueDock rows={[{ ...row(), message: { ...row().message, content: [{ type: 'text', text: text.slice(0, 8) }, { type: 'text', text: text.slice(8) }] } }]} submissions={[]} running edit={edit} />);
    fireEvent.click(screen.getByRole('button', { name: 'Edit' }));
    const editor = screen.getByRole('textbox', { name: 'Edit queued message' }) as HTMLTextAreaElement;
    expect(editor.value).toBe(text);
    // Shift+Enter does not save.
    expect(fireEvent.keyDown(editor, { key: 'Enter', shiftKey: true })).toBe(true);
    expect(edit).not.toHaveBeenCalled();
    // IME composition Enter does not save.
    fireEvent.change(editor, { target: { value: `${text}\nline five` } });
    fireEvent.keyDown(editor, { key: 'Enter', isComposing: true });
    fireEvent.keyDown(editor, { key: 'Enter', keyCode: 229 });
    expect(edit).not.toHaveBeenCalled();
    await act(async () => { fireEvent.keyDown(editor, { key: 'Enter' }); });
    expect(edit).toHaveBeenCalledExactlyOnceWith({ sequence: '7', revision: '0' }, `${text}\nline five`);
  });

  it('renders durable image thumbnails and file attachments in block order with ArtifactContext', async () => {
    const server = new Server(); await server.attached('A');
    const resources = new ArtifactResources(server.client, 'A');
    const read = vi.spyOn(resources, 'read').mockResolvedValue('blob:durable-thumb');
    const release = vi.spyOn(resources, 'release').mockImplementation(() => {});
    try {
      const mixed = {
        ...row(),
        message: {
          ...row().message,
          content: [
            { type: 'file' as const, artifact_id: 'f-1', name: 'report.csv' },
            { type: 'image' as const, artifact_id: 'img-1', alt: 'Preview shot' },
            { type: 'text' as const, text: 'See both' },
          ],
        },
      };
      const ui = render(<ArtifactContext.Provider value={resources}><QueueDock rows={[mixed]} submissions={[]} running /></ArtifactContext.Provider>);
      const img = await ui.findByRole('img', { name: 'Preview shot' });
      expect(img.getAttribute('src')).toBe('blob:durable-thumb');
      expect(read).toHaveBeenCalledWith('img-1');
      const fileChip = ui.getByLabelText('Queued file report.csv');
      expect(fileChip.textContent).toBe('report.csv');
      expect(fileChip.textContent).not.toContain('undefined');
      expect(fileChip.parentElement?.children[0]).toBe(fileChip);
      expect(fileChip.parentElement?.children[1]).toBe(img);
      expect(ui.getByText('See both')).toBeTruthy();
      ui.unmount();
      expect(release).toHaveBeenCalledWith('blob:durable-thumb');
    } finally {
      resources.dispose();
      server.client.disconnect();
    }
  });

  it('retains the thumbnail placeholder when an image read fails and shows hover tooltips on enabled actions', async () => {
    vi.useFakeTimers();
    const server = new Server(); await server.attached('A');
    const resources = new ArtifactResources(server.client, 'A');
    vi.spyOn(resources, 'read').mockRejectedValue(new Error('denied'));
    try {
      const withImage = { ...row(), message: { ...row().message, content: [{ type: 'image' as const, artifact_id: 'img-err' }] } };
      const ui = render(<ArtifactContext.Provider value={resources}><QueueDock rows={[withImage]} submissions={[]} running remove={vi.fn()} /></ArtifactContext.Provider>);
      await act(async () => { await Promise.resolve(); });
      expect(ui.container.querySelector('img')).toBeNull();
      const removeButton = ui.getByRole('button', { name: 'Remove' });
      fireEvent.mouseEnter(removeButton);
      act(() => { vi.advanceTimersByTime(500); });
      expect(ui.getByRole('tooltip').textContent).toBe('Remove');
    } finally {
      resources.dispose();
      server.client.disconnect();
      vi.useRealTimers();
    }
  });

  it('live user uploads carry real File name, size, and safe raster thumbnails across Sending and durable QueueDock rows, releasing on drain', async () => {
    const pngBytes = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6pAAAAABJRU5ErkJggg=='), c => c.charCodeAt(0));
    const pngFile = new File([pngBytes], 'shot.png', { type: 'image/png' });
    const txtFile = new File(['hello world'], 'notes.txt', { type: 'text/plain' });
    const svgFile = new File(['<svg/>'], 'icon.svg', { type: 'image/svg+xml' });
    let nextBlob = 0;
    const origCreateObjectUrl = URL.createObjectURL;
    const origRevokeObjectUrl = URL.revokeObjectURL;
    const createObjectUrl = vi.fn(() => `blob:test-url-${++nextBlob}`);
    const revokeObjectUrl = vi.fn();
    Object.assign(URL, { createObjectURL: createObjectUrl, revokeObjectURL: revokeObjectUrl });
    let ui: ReturnType<typeof render> | undefined;
    try {
      ui = await mount(running(snapshot()));
      const fileInput = ui.container.querySelector('input[type="file"]') as HTMLInputElement;
      const uploadedBatches: Array<{ receipt: { session_id: string; batch_id: string; token: string }; file: { batch_id: string; name: string } }> = [];
      for (const file of [pngFile, txtFile, svgFile]) {
        const before = uploadedBatches.length;
        await act(async () => {
          fireEvent.change(fileInput, { target: { files: [file] } });
        });
        await waitFor(() => {
          expect(server.requests.filter(r => r.request.method === 'session/uploadStatus').length).toBeGreaterThan(before);
        });
        const prepareReq = server.requests.filter(r => r.request.method === 'session/uploadPrepare').at(-1)!;
        const batchId = (prepareReq.request.params as { operation_id: string }).operation_id;
        uploadedBatches.push({
          receipt: { session_id: 'A', batch_id: batchId, token: 'file-0' },
          file: { batch_id: batchId, name: file.name },
        });
      }
      // Wait for all draft attachment chips to settle so Queue button is enabled.
      await waitFor(() => {
        expect(screen.getByAltText('shot.png')).toBeTruthy();
        expect(screen.getByText('notes.txt')).toBeTruthy();
        expect(screen.getByAltText('icon.svg')).toBeTruthy();
      });

      // Hold inbound/queue to observe Sending row with uploaded metadata before ACK.
      server.held.add('inbound/queue');
      fireEvent.change(screen.getByLabelText('Message'), { target: { value: 'Check files' } });
      await waitFor(() => {
        expect((screen.getByRole('button', { name: 'Queue' }) as HTMLButtonElement).disabled).toBe(false);
      });
      await act(async () => {
        fireEvent.click(screen.getByRole('button', { name: 'Queue' }));
      });
      await server.waitFor('inbound/queue', 1);

      const queueRegion = dock('Queue');
      expect(within(queueRegion).getByText('Sending…')).toBeTruthy();
      expect(within(queueRegion).getByRole('img', { name: 'shot.png' })).toBeTruthy();
      const notesChip = within(queueRegion).getByLabelText('Queued file notes.txt');
      expect(notesChip.textContent).toContain('notes.txt');
      expect(notesChip.textContent).toContain('11B');
      const svgChip = within(queueRegion).getByLabelText('Queued file icon.svg');
      expect(svgChip.textContent).toContain('icon.svg');
      expect(svgChip.textContent).toContain('6B');
      expect(within(queueRegion).queryByRole('img', { name: 'icon.svg' })).toBeNull();
      expect(within(queueRegion).getByText('Check files')).toBeTruthy();

      // Native accepts and stores durable uploaded_file blocks; Sending row retires cleanly.
      server.held.delete('inbound/queue');
      const queuedReq = server.requests.filter(r => r.request.method === 'inbound/queue').at(-1)!;
      const durableRow = {
        revision: '0',
        sequence: '10',
        message: {
          id: 'msg-durable-files',
          source: 'human' as const,
          kind: 'queued_message' as const,
          content: [
            ...uploadedBatches.map(entry => ({ type: 'uploaded_file' as const, ...entry.file })),
            { type: 'text' as const, text: 'Check files' },
          ],
        },
      };
      await act(async () => {
        server.socket.deliver({ jsonrpc: '2.0', id: queuedReq.request.id, result: { type: 'inbound_accepted', message_id: 'msg-durable-files', inbound_sequence: '10' } });
        await server.update('A', running({ ...snapshot(), inbound: { pending: [durableRow] } }));
      });
      await waitFor(() => {
        expect(within(dock('Queue')).queryByText('Sending…')).toBeNull();
      });
      expect(within(dock('Queue')).getByRole('img', { name: 'shot.png' })).toBeTruthy();
      expect(within(dock('Queue')).getByLabelText('Queued file notes.txt').textContent).toContain('11B');
      expect(within(dock('Queue')).getByLabelText('Queued file icon.svg').textContent).toContain('6B');
      expect(within(dock('Queue')).queryByRole('img', { name: 'icon.svg' })).toBeNull();
      const createdThumbUrls = createObjectUrl.mock.results.map(result => result.value as string);
      expect(createdThumbUrls.length).toBeGreaterThan(0);

      // Draining the queue releases the upload metadata and revokes thumbnail URLs.
      await update(running(withQueue([])));
      await waitFor(() => {
        expect(region('Queue')).toBeNull();
      });
      for (const createdUrl of createdThumbUrls) {
        expect(revokeObjectUrl).toHaveBeenCalledWith(createdUrl);
      }
      expect(server.client.uploadAttachmentMetadata('A', { receipt: uploadedBatches[0].receipt })).toBeUndefined();
      expect(server.client.uploadAttachmentMetadata('A', { file: uploadedBatches[0].file })).toBeUndefined();
    } finally {
      ui?.unmount();
      Object.assign(URL, { createObjectURL: origCreateObjectUrl, revokeObjectURL: origRevokeObjectUrl });
    }
  });

  it('uses authorized native names without fake sizes on restored attachments and releases upload metadata on session release', async () => {
    const pngBytes = Uint8Array.from(atob('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6pAAAAABJRU5ErkJggg=='), c => c.charCodeAt(0));
    const restoredDurable = {
      ...row(),
      message: {
        ...row().message,
        content: [
          { type: 'uploaded_file' as const, batch_id: 'restored-batch', name: 'restored.pdf' },
          { type: 'text' as const, text: 'From prior run' },
        ],
      },
    };
    const restoredSending = [{
      messageId: 'restored-sending',
      sendingInputId: 'in-restored',
      placement: 'queued' as const,
      content: [
        { type: 'upload' as const, session_id: 'A', batch_id: 'restored-b2', token: 't1' },
        { type: 'text' as const, text: 'Restored draft' },
      ],
    }];
    const dockUi = render(<QueueDock rows={[restoredDurable]} submissions={restoredSending} running />);
    fireEvent.click(within(dockUi.getByRole('region', { name: 'Queue' })).getByRole('button', { expanded: false }));
    const chip = dockUi.getByLabelText('Queued file restored.pdf');
    expect(chip.textContent).toBe('restored.pdf');
    expect(chip.textContent).not.toContain('0B');
    expect(dockUi.queryByRole('img')).toBeNull();
    expect(dockUi.getByText(/1 attachment/)).toBeTruthy();
    dockUi.unmount();

    // Live upload followed by session release clears stored metadata immediately.
    await server.attached('A');
    const pngFile = new File([pngBytes], 'release.png', { type: 'image/png' });
    const [uploaded] = await server.client.upload('A', [pngFile]);
    expect(server.client.uploadAttachmentMetadata('A', { receipt: uploaded.receipt })?.name).toBe('release.png');
    expect(server.client.uploadAttachmentMetadata('A', { file: uploaded.file })?.bytes).toBe(pngFile.size);
    await server.client.release('A');
    expect(server.client.uploadAttachmentMetadata('A', { receipt: uploaded.receipt })).toBeUndefined();
    expect(server.client.uploadAttachmentMetadata('A', { file: uploaded.file })).toBeUndefined();
  });

  it('empty-draft Ctrl/Cmd+Enter in ConversationSeat calls client.steerInbound(sessionId) without expected and advertises the contextual hint', async () => {
    await mount(running(withQueue([inbound('1', 'One'), inbound('2', 'Two')])));
    const steerInbound = vi.spyOn(server.client, 'steerInbound').mockResolvedValue({ status: 'known', outcome: { status: 'applied', read_cut: readCut() }, observed: true });
    const textarea = screen.getByLabelText('Message');
    expect(textarea.getAttribute('placeholder')).toBe('Cmd/Ctrl+Enter steers all queued messages');
    await act(async () => { fireEvent.keyDown(textarea, { key: 'Enter', metaKey: true }); });
    expect(steerInbound).toHaveBeenCalledTimes(1);
    expect(steerInbound).toHaveBeenCalledWith('A');
    expect(steerInbound.mock.calls[0]).toHaveLength(1);

    // Non-empty draft shows ordinary guidance and steers only the draft.
    fireEvent.change(textarea, { target: { value: 'New steer' } });
    expect(textarea.getAttribute('placeholder')).toBe('Give this Session a task…');
    await act(async () => { fireEvent.keyDown(textarea, { key: 'Enter', ctrlKey: true }); });
    await server.waitFor('turn/steer', 1);
    expect(steerInbound).toHaveBeenCalledTimes(1);

    // Clicking per-row Steer in QueueDock calls client.steerInbound(sessionId, expected).
    fireEvent.click(within(dock('Queue')).getByRole('button', { expanded: false }));
    const steerButtons = within(dock('Queue')).getAllByRole('button', { name: 'Steer queued message' });
    await act(async () => { fireEvent.click(steerButtons[0]); });
    expect(steerInbound).toHaveBeenLastCalledWith('A', { sequence: '1', revision: '0' });
  });
});

it('T08/T11 deferred Goal editing loads the original field and never submits its preview', async () => {
  const original = 'objective beyond the finite header '.repeat(100).trim(), preview = original.slice(0, 512);
  const current = goal({ objective: preview, objective_deferred: true, fields: [{ reference: goal().reference, path: ['current', 'objective'] }] });
  let release!: (value: string) => void;
  const loadObjective = vi.fn(() => new Promise<string>(resolve => { release = resolve; }));
  const mutate = vi.fn(async (): Promise<GoalControlOutcome> => ({ status: 'applied', observed: true }));
  render(<GoalDock state={{ goal: current }} observation={{}} disabled={false} mutate={mutate} loadObjective={loadObjective}/>);
  fireEvent.click(goalButton('Edit goal objective'));
  expect(loadObjective).toHaveBeenCalledExactlyOnceWith(current.reference);
  expect(within(dock('Goal')).queryByRole('textbox')).toBeNull();
  expect(mutate).not.toHaveBeenCalled();
  await act(async () => { release(original); });
  expect(within(dock('Goal')).getByRole('textbox')).toHaveProperty('value', original);
  fireEvent.click(goalButton('Save goal objective'));
  expect(mutate).toHaveBeenCalledExactlyOnceWith(current.reference, { action: 'edit', objective: original });
});

it('T11 a late Goal field read cannot open an editor for a newer revision', async () => {
  const current = goal({ objective: 'preview', objective_deferred: true });
  let release!: (value: string) => void;
  const loadObjective = vi.fn(() => new Promise<string>(resolve => { release = resolve; }));
  const mutate = vi.fn(async (): Promise<GoalControlOutcome> => ({ status: 'applied', observed: true }));
  const ui = render(<GoalDock state={{ goal: current }} observation={{}} disabled={false} mutate={mutate} loadObjective={loadObjective}/>);
  fireEvent.click(goalButton('Edit goal objective'));
  ui.rerender(<GoalDock state={{ goal: { ...current, reference: { ...current.reference, revision: '4' } } }} observation={{}} disabled={false} mutate={mutate} loadObjective={loadObjective}/>);
  await act(async () => { release('obsolete original'); });
  expect(within(dock('Goal')).queryByRole('textbox')).toBeNull();
  expect(mutate).not.toHaveBeenCalled();
});

it('T07/T11 losing Goal Show delivery is an observation failure without an uncertain mutation', async () => {
  server.snapshots.set('A', withGoal(goal()));
  await server.attached('A');
  server.held.add('goal/control');
  const work = server.client.request({ method: 'goal/control', params: { target: server.client.target('A'), control: { action: 'show' } } }, 'domain_page').catch(error => error);
  await server.waitFor('goal/control', 1);
  act(() => server.socket.close());
  expect(await work).toBeInstanceOf(Error);
  expect(server.client.getSnapshot().uncertain.some(value => value.method === 'goal/control')).toBe(false);
  expect(goalControls()).toEqual([{ action: 'show' }]);
});
