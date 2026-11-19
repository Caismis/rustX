import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { ConversationTotals } from '../src/app/agent/ConversationLive';
import { Server, snapshot } from './fixture';
import { agentMetrics } from './agent-statistics-fixture';
import type { RuntimeClientSnapshot } from '../../protocol/app-server/v80';

let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); });

function measured(id: string): RuntimeClientSnapshot {
  const state = snapshot(id);
  state.transcript = { ...state.transcript, statistics: agentMetrics.statistics };
  state.context = { compaction_in_progress: false, compaction_count: 0, occupancy: agentMetrics.occupancy ?? null };
  return state;
}
const dock = () => document.querySelector('[data-composer-dock]');

it('the composer reserves its totals row without displaying stale readings across a checkpoint handoff', async () => {
  server = new Server();
  server.snapshots.set('A', measured('A'));
  await server.attached('A');
  render(<ConversationTotals client={server.client} sessionId="A"/>);
  expect(dock()).toBeTruthy();
  await act(async () => { await server.client.release('A'); });
  expect(dock()).toBeTruthy();
  server.loaded.delete('A');
  server.held.add('session/checkpoint');
  server.held.add('session/statistics');
  let attach!: Promise<void>;
  await act(async () => { attach = server.client.attach('A'); void attach.catch(() => {}); });
  await server.waitFor('session/checkpoint', 2);
  await act(async () => {});
  const view = server.client.getSnapshot().views.A;
  expect(view.attachment).toBe('attached');
  expect(view.observationState).toBe('checkpointLoading');
  expect(view.snapshot).toBeUndefined();
  expect(view.checkpoint?.snapshot.conversation_id).toBe(view.target?.conversation_id);
  expect(dock()?.getAttribute('aria-hidden')).toBe('true');
  expect(dock()?.textContent).toBe('');
  await act(async () => {
    server.held.delete('session/checkpoint');
    server.held.delete('session/statistics');
    server.reply(await server.waitFor('session/checkpoint', 2));
    await attach;
  });
  expect(dock()).toBeTruthy();
});

it('a different Conversation never lends its retained checkpoint totals', async () => {
  server = new Server();
  server.snapshots.set('A', measured('A'));
  server.nodeSnapshots.set('right', measured('right'));
  await server.attached('A');
  render(<ConversationTotals client={server.client} sessionId="A"/>);
  expect(dock()).toBeTruthy();
  await act(async () => { await server.client.switchNode('A', 'right'); });
  server.held.add('session/checkpoint');
  server.held.add('session/statistics');
  let attach!: Promise<void>;
  await act(async () => { attach = server.client.attach('A'); void attach.catch(() => {}); });
  await server.waitFor('session/checkpoint', 2);
  await act(async () => {});
  const view = server.client.getSnapshot().views.A;
  expect(view.target?.conversation_id).toBe('conversation-right');
  expect(view.checkpoint?.snapshot.conversation_id).toBe('conversation-A');
  expect(view.snapshot).toBeUndefined();
  expect(dock()).toBeNull();
  await act(async () => {
    server.held.delete('session/checkpoint');
    server.held.delete('session/statistics');
    server.reply(await server.waitFor('session/checkpoint', 2));
    await attach;
  });
  expect(dock()).toBeTruthy();
});
