import { act, cleanup, fireEvent, render, renderHook, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { CheckpointPage, RuntimeClientGoalSnapshot } from '../../protocol/app-server/v61';
import { useComposerContent } from '../src/app/agent/useComposerContent';
import { TodoDock } from '../src/app/composer/TodoDock';
import { todoDock } from '../src/bindings/composer-context';
import type { SessionView } from '../src/client/contracts';
import { Server, snapshot } from './fixture';
let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); });
const tasks = [1, 2, 3].map(id => ({ id: String(id), subject: `Original task ${id}`, status: 'pending' as const }));
function Dock({ view }: { view: SessionView }) {
 const content = useComposerContent(server.client, view);
 return <TodoDock state={todoDock(content.snapshot, content.todoLoaded ? undefined : view.checkpoint?.collections.find(row => row.domain === 'todos.tasks'))}/>;
}
async function hydratedDock() {
 server = new Server(); server.snapshots.set('A', { ...snapshot(), todos: { tasks: [], next_id: '4' } }); await server.attached('A');
 const view = server.client.getSnapshot().views.A, cut = view.checkpoint!.read_cut;
 view.checkpoint!.collections.push({ domain: 'todos.tasks', total: '3', loaded: '0', revision: '1', next_offset: '0' });
 server.handlers.set('conversation/checkpointPage', () => ({ type: 'checkpoint_page', page: { read_cut: cut, cursor: cut.revision, domain: 'todos.tasks', total: '3', next_offset: null, collections: [], items: tasks.map((value, index) => ({ offset: String(index), identity: value.id, value, content: [], deferred: false })) } }));
 const ui = render(<Dock view={view}/>);
 await waitFor(() => expect(screen.queryByText('Loading')).toBeNull());
 fireEvent.click(screen.getByRole('button', { name: /To-dos/ }));
 await screen.findByText('Original task 3');
 return { ui, view, rows: [...ui.container.querySelectorAll('[data-task-id]')] };
}
it('keeps expanded hydrated Todo rows intact while an independent Goal refresh waits for its content', async () => {
 const { ui, view, rows } = await hydratedDock();
 server.held.add('conversation/checkpointPage'); server.held.add('conversation/checkpointValueRead');
 const goal: RuntimeClientGoalSnapshot = { reference: { id: 'goal-1', revision: '4' }, objective: 'Goal changed independently', objective_deferred: false, phase: 'blocked', blocked_reason: 'preview', blocked_reason_deferred: true, autonomous_round_budget: 4, autonomous_rounds_consumed: 1, origin: { kind: 'runtime_control' }, last_round_message_id_deferred: false, fields: [] };
 ui.rerender(<Dock view={{ ...view, snapshot: { ...view.snapshot!, goal: { current: goal } } }}/>);
 expect([...ui.container.querySelectorAll('[data-task-id]')]).toEqual(rows);
 for (const row of rows) expect(row.isConnected).toBe(true);
 expect(screen.queryByRole('status')).toBeNull();
 await server.waitFor('conversation/checkpointValueRead', 1);
 expect(server.requests.filter(row => row.request.method === 'conversation/checkpointPage')).toHaveLength(1);
});
it('keeps hydrated Todos visible as refreshing and replaces the list atomically on a Todo revision change', async () => {
 const { ui, view, rows } = await hydratedDock(); server.held.add('conversation/checkpointPage');
 const checkpoint = { ...view.checkpoint!, collections: [{ domain: 'todos.tasks', total: '4', loaded: '0', revision: '2', next_offset: '0' }] };
 ui.rerender(<Dock view={{ ...view, checkpoint }}/>);
 const request = await server.waitFor('conversation/checkpointPage', 2);
 expect([...ui.container.querySelectorAll('[data-task-id]')]).toEqual(rows);
 expect(screen.getByRole('status').textContent).toContain('Loading');
 const next = [...tasks, { id: '4', subject: 'New task after refresh', status: 'pending' as const }];
 await act(async () => server.socket.success(request, { type: 'checkpoint_page', page: { read_cut: checkpoint.read_cut, cursor: checkpoint.read_cut.revision, domain: 'todos.tasks', total: '4', next_offset: null, collections: [], items: next.map((value, index) => ({ offset: String(index), identity: value.id, value, content: [], deferred: false })) } }));
 await screen.findByText('New task after refresh');
 expect([...ui.container.querySelectorAll('[data-task-id]')].slice(0, 3)).toEqual(rows);
 expect(screen.queryByRole('status')).toBeNull();
});
it('retains a complete inline Todo list when an unrelated capture budget defers the same Todo revision', async () => {
 server = new Server(); server.snapshots.set('A', { ...snapshot(), todos: { tasks, next_id: '4' } }); await server.attached('A');
 const view = server.client.getSnapshot().views.A;
 view.checkpoint!.collections.push({ domain: 'todos.tasks', total: '3', loaded: '3', revision: '1', next_offset: null });
 const ui = render(<Dock view={view}/>); fireEvent.click(screen.getByRole('button', { name: /To-dos/ }));
 const rows = [...ui.container.querySelectorAll('[data-task-id]')];
 const checkpoint = { ...view.checkpoint!, collections: [{ domain: 'todos.tasks', total: '3', loaded: '0', revision: '1', next_offset: '0' }] };
 server.held.add('conversation/checkpointPage');
 ui.rerender(<Dock view={{ ...view, checkpoint, snapshot: { ...view.snapshot!, todos: { tasks: [], next_id: '4' } } }}/>);
 expect([...ui.container.querySelectorAll('[data-task-id]')]).toEqual(rows);
 expect(screen.queryByRole('status')).toBeNull();
 expect(server.requests.filter(row => row.request.method === 'conversation/checkpointPage')).toHaveLength(0);
});
it('fills bounded Todo and Goal headers in the existing docks without changing native state', async () => {
 server = new Server();
 const goal: RuntimeClientGoalSnapshot = { reference: { id: 'goal-1', revision: '3' }, objective: 'preview', objective_deferred: true, phase: 'blocked', blocked_reason: 'preview', blocked_reason_deferred: true, autonomous_round_budget: 4, autonomous_rounds_consumed: 1, origin: { kind: 'runtime_control' }, last_round_message_id_deferred: false, fields: [] };
 server.snapshots.set('A', { ...snapshot(), todos: { tasks: tasks.slice(0, 1), next_id: '4' }, goal: { current: goal } });
 await server.attached('A'); const view = server.client.getSnapshot().views.A, cut = view.checkpoint!.read_cut;
 view.checkpoint!.collections.push({ domain: 'todos.tasks', total: '3', loaded: '1', revision: '1', next_offset: '1' });
 server.handlers.set('conversation/checkpointPage', request => {
  if (request.method !== 'conversation/checkpointPage') throw Error('page');
  expect(request.params.cut).toEqual(cut); expect(request.params.domain).toBe('todos.tasks');
  return { type: 'checkpoint_page', page: { read_cut: cut, cursor: cut.revision, domain: 'todos.tasks', total: '3', next_offset: null, collections: [], items: tasks.map((value, index) => ({ offset: String(index), identity: value.id, value, content: [], deferred: false })) } };
 });
 const objective = 'Original goal objective. '.repeat(80).trim(), reason = 'Original blocking reason.';
 server.handlers.set('conversation/checkpointValueRead', request => {
  if (request.method !== 'conversation/checkpointValueRead') throw Error('field'); const params = request.params;
  expect(params.cut).toEqual(cut); const text = params.path[1] === 'objective' ? objective : reason, bytes = new TextEncoder().encode(text);
  return { type: 'checkpoint_value_chunk', chunk: { cut, domain: params.domain, item_offset: 0, path: params.path, offset: 0, readable_end: bytes.length, next_offset: null, representation: 'utf8_text', bytes_base64: btoa(String.fromCharCode(...bytes)) } };
 });
 const ui = renderHook(() => useComposerContent(server.client, view));
 await waitFor(() => expect(ui.result.current.snapshot?.goal?.current?.objective).toBe(objective));
 expect(ui.result.current.snapshot?.goal?.current?.blocked_reason).toBe(reason); expect(ui.result.current.snapshot?.todos).toEqual({ tasks, next_id: '4' });
 expect(ui.result.current.todoLoaded).toBe(true); expect(server.client.getSnapshot().views.A.snapshot?.goal?.current?.objective).toBe('preview');
 expect(server.requests.some(row => row.request.method === 'goal/control' || row.request.method === 'session/setModel')).toBe(false);
});
it('a late Todo continuation cannot repaint another selected conversation', async () => {
 server = new Server(); server.snapshots.set('A', { ...snapshot(), todos: { tasks: [], next_id: '4' } }); await server.attached('A', 'B');
 const first = server.client.getSnapshot().views.A, replacement = server.client.getSnapshot().views.B, cut = first.checkpoint!.read_cut;
 first.checkpoint!.collections.push({ domain: 'todos.tasks', total: '3', loaded: '0', revision: '1', next_offset: '0' }); server.held.add('conversation/checkpointPage');
 const ui = renderHook(({ view }) => useComposerContent(server.client, view), { initialProps: { view: first } }); const request = await server.waitFor('conversation/checkpointPage', 1);
 ui.rerender({ view: replacement });
 const page: CheckpointPage = { read_cut: cut, cursor: cut.revision, domain: 'todos.tasks', total: '3', next_offset: null, collections: [], items: tasks.map((value, index) => ({ offset: String(index), identity: value.id, value, content: [], deferred: false })) };
 await act(async () => server.socket.success(request, { type: 'checkpoint_page', page }));
 expect(ui.result.current.snapshot).toBe(replacement.snapshot); expect(ui.result.current.todoLoaded).toBe(false);
});
