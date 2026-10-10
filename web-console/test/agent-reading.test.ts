import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { createElement } from 'react';
import { SubagentChat } from '../src/app/agent/SubagentChat';
import { installTurnNavigatorObserver } from './turn-navigator-fixture';
import type { ConversationTurn, MethodResult, RuntimeClientSnapshot } from '../../protocol/app-server/v42';
import { AgentReading } from '../src/client/agent-reading';
import { turnKey } from '../src/client/transcript';
import { Server, snapshot } from './fixture';

const child = 'conversation-worker';
const cut = { conversation_id: child, journal: '300', transcript: '300', mutation_revision: '0' };
const turn = (n: number): ConversationTurn => ({ id: { conversation_id: child, attempt_id: `child-${n}` }, ordinal: n, cursor: String(n), prompt: `Task ${n}`, response: `Report ${n}` });
const entry = (n: number) => ({ cursor: String(n), item: { type: 'message' as const, message: { role: 'assistant' as const, id: `reply-${n}`, content: [{ type: 'text' as const, text: `Report ${n}` }] } }, turn_process: { ...turn(n).id, control_cursor: String(n), final_message_id: `reply-${n}`, message_count: 1, tool_call_count: 0, outcome: 'completed' as const } });
const live = (n: number): RuntimeClientSnapshot => ({ ...snapshot(), conversation_id: child, transcript: { entries: [entry(n)], next_cursor: String(n) } });
const window = (n: number): MethodResult => ({ type: 'transcript_window', window: { cut, page: { entries: [entry(n)], next_cursor: String(n) }, target: turn(n).id, target_cursor: String(n), newer_cursor: String(n) } });
let server: Server, reader: AgentReading;
afterEach(() => { cleanup(); vi.unstubAllGlobals(); reader?.dispose(); server?.client.disconnect(); });
async function setup() {
  server = new Server(); await server.attached('A');
  reader = new AgentReading(server.client, server.client.target('A'), 'agent-worker', child,
    () => server.client.isAttachmentObservationCurrent('A', server.client.getSnapshot().views.A.attachmentObservation));
  server.handlers.set('agent/turns', request => {
    if (request.method !== 'agent/turns') throw Error('method');
    const offset = request.params.offset ?? 128;
    return { type: 'conversation_turns', page: { cut, total: 130, offset, turns: Array.from({ length: Math.min(64, 130 - offset) }, (_, index) => turn(offset + index + 1)) } };
  });
  server.handlers.set('agent/transcript', request => {
    if (request.method !== 'agent/transcript' || request.params.at.type !== 'turn') throw Error('turn window');
    return window(Number(request.params.at.id.attempt_id.slice(6)));
  });
  reader.observe(live(130)); await reader.refreshTurns();
}
it('pages every native child turn, resolves an unloaded ordinal, keeps a historical window independent of live updates and returns to latest', async () => {
  await setup();
  expect(reader.getSnapshot().outline?.total).toBe(130);
  expect(reader.getSnapshot().outline?.turns.map(turn => turn.ordinal)).toEqual([129, 130]);
  expect(await reader.navigate(1)).toEqual(turn(1));
  const request = server.requests.find(row => row.request.method === 'agent/transcript')!.request;
  expect(request).toMatchObject({ params: { target: server.client.target('A'), agent_id: 'agent-worker', at: { type: 'turn', id: turn(1).id, cut }, limit: 64 } });
  reader.observe(live(131));
  expect(reader.getSnapshot().history?.window?.target).toEqual(turn(1).id);
  expect(reader.getSnapshot().history?.page.entries?.map(entry => entry.cursor)).toEqual(['1']);
  reader.returnToLatest();
  expect(reader.getSnapshot().history?.window).toBeUndefined();
  expect(reader.getSnapshot().history?.page.entries?.map(entry => entry.cursor)).toEqual(['131']);
});
it('a newer selection owns its window even when the earlier native response arrives last', async () => {
  await setup(); server.held.add('agent/transcript');
  const first = reader.navigate(1); const a = await server.waitFor('agent/transcript', 1);
  const second = reader.navigate(turn(2)); const b = await server.waitFor('agent/transcript', 2);
  server.socket.success(b, window(2)); expect(await second).toEqual(turn(2));
  server.socket.success(a, window(1)); expect(await first).toBe(false);
  expect(reader.getSnapshot().history?.window?.target).toEqual(turn(2).id);
  expect(reader.getSnapshot().pending).toBeUndefined();
});
it('manual scrolling and disposal retire pending navigation without replacing the reading window', async () => {
  await setup(); server.held.add('agent/transcript'); let gesture = true;
  const request = reader.navigate(1, () => gesture); const held = await server.waitFor('agent/transcript', 1);
  gesture = false; server.socket.success(held, window(1)); expect(await request).toBe(false);
  expect(reader.getSnapshot().history?.window).toBeUndefined();
  const later = reader.navigate(turn(2)); const pending = await server.waitFor('agent/transcript', 2);
  reader.dispose(); const before = reader.getSnapshot(); server.socket.success(pending, window(2));
  expect(await later).toBe(false); expect(reader.getSnapshot()).toBe(before);
});
it('rejects foreign child cuts and invalid target locations, allowing an explicit reload', async () => {
  await setup();
  server.handlers.set('agent/transcript', () => ({ type: 'transcript_window', window: { cut: { ...cut, conversation_id: 'other-child' }, page: { entries: [entry(1)] }, target: turn(1).id, target_cursor: '1' } }));
  expect(await reader.navigate(1)).toBe(false);
  expect(reader.getSnapshot().navigationError).toContain('Invalid native child Turn window');
  expect(reader.getSnapshot().history?.window).toBeUndefined();
  server.handlers.set('agent/transcript', () => window(1));
  await reader.refreshTurns(); expect(await reader.navigate(1)).toEqual(turn(1));
  expect(reader.getSnapshot().navigationError).toBeUndefined();
});
it('reads older/newer finite windows at the child cut and bounds retained entries', async () => {
  await setup(); await reader.navigate(1);
  server.handlers.set('agent/transcript', request => {
    if (request.method !== 'agent/transcript') throw Error('method');
    const n = request.params.at.type === 'newer' ? 2 : 0;
    return { type: 'transcript_window', window: { cut, page: { entries: [entry(n)], next_cursor: n ? '1' : null }, newer_cursor: n ? null : '1' } };
  });
  await reader.loadLater();
  expect(reader.getSnapshot().history?.page.entries?.map(entry => entry.cursor)).toEqual(['1', '2']);
  const newest = server.requests.at(-1)!.request;
  expect(newest).toMatchObject({ method: 'agent/transcript', params: { at: { type: 'newer', after: '1', cut } } });
  await reader.loadEarlier();
  expect(reader.getSnapshot().history?.page.entries?.map(entry => entry.cursor)).toEqual(['0', '1', '2']);
  reader.returnToLatest();
  for (let n = 132; n < 500; n++) reader.observe({ ...live(n), transcript: { entries: [entry(n - 1), entry(n)], next_cursor: String(n - 1) } });
  expect(reader.getSnapshot().history?.page.entries!.length).toBeLessThanOrEqual(256);
  expect(turnKey(turn(1).id)).not.toBe(turnKey(turn(130).id));
});
it('parent detachment rejects pending child outline adoption', async () => {
  await setup(); server.held.add('agent/turns');
  const reading = reader.refreshTurns(); const held = await server.waitFor('agent/turns', server.requests.filter(row => row.request.method === 'agent/turns').length);
  await server.client.release('A'); const before = reader.getSnapshot();
  server.socket.success(held, { type: 'conversation_turns', page: { cut, total: 1, offset: 0, turns: [turn(1)] } });
  await reading; expect(reader.getSnapshot()).toBe(before);
});

it('resident child conversations keep their historical location across hide and show', async () => {
  await setup(); installTurnNavigatorObserver(600);
  const props = { client: server.client, sessionId: 'A', admission: server.client.getSnapshot().views.A.attachmentObservation,
    agent: { title: 'Worker', agent: 'Worker', agent_id: 'agent-worker', parent_agent_id: 'root', child_conversation_id: child,
      activation_id: 'activation', state: 'inactive' as const, activation_state: 'succeeded' as const, started_at: '2026-10-10T00:00:00Z', definition_digest: 'd', profile_digest: 'p',
      observation: { attempt_id: null, revision: '1', activity: { type: 'awaiting_activity' as const }, counters: { model_requests: 0, model_retries: 0, tool_executions: 0 } },
      workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' as const }, resource_state: 'none' as const } }, snapshot: live(130) };
  let ui!: ReturnType<typeof render>;
  await act(async () => { ui = render(createElement(SubagentChat, { ...props, visible: true })); });
  const rail = ui.getByRole('navigation', { name: 'Turn navigation' }).firstElementChild as HTMLElement;
  await act(async () => { rail.scrollTop = 0; fireEvent.scroll(rail); });
  await act(async () => { fireEvent.click(ui.getByRole('button', { name: 'Load and jump to turn 1' })); });
  expect(ui.getByText('Report 1')).toBeTruthy();
  await act(async () => { ui.rerender(createElement(SubagentChat, { ...props, visible: false })); });
  await act(async () => { ui.rerender(createElement(SubagentChat, { ...props, visible: true })); });
  expect(ui.getByText('Report 1')).toBeTruthy();
  expect(server.requests.filter(row => row.request.method === 'agent/transcript')).toHaveLength(1);
});
