import { afterEach, expect, it } from 'vitest';
import type { RuntimeClientTranscriptEntry } from '../../protocol/app-server/v57';
import { Server, snapshot, readingWindow } from './fixture';
import { transcriptWindowReady, traceWindowReady } from '../src/client/history-policy';
import { traceRecord } from './trace-fixture';
const entry = (n: number): RuntimeClientTranscriptEntry => ({ cursor: String(n), item: { type: 'message', message: { role: 'assistant', id: `m${n}`, content: [{ type: 'text', text: `Message ${n}` }] } }, turn_process: {
 conversation_id: 'conversation-A', attempt_id: `attempt-${Math.floor(n / 10)}`, control_cursor: String(n), outcome: 'completed', message_count: 1, tool_call_count: 0,
 final_message_locator: null, final_message_id_deferred: false, final_message_id_source: null,
} });
let server: Server;
afterEach(() => server?.client.disconnect());
it('follows DSH message and Turn thresholds, without treating model-only Context as conversation messages', () => {
 const rows = Array.from({ length: 50 }, (_, n) => entry(n + 1));
 expect(transcriptWindowReady(rows)).toBe(true); expect(transcriptWindowReady(rows.slice(0, 49))).toBe(false);
 expect(transcriptWindowReady(rows.map(row => ({ ...row, turn_process: { ...row.turn_process!, attempt_id: 'one-turn' } })))).toBe(false);
 expect(transcriptWindowReady(rows.map(row => ({ ...row, item: { type: 'message', message: { role: 'user', id: `context-${row.cursor}`, source: 'runtime', kind: { context: 'native_environment' }, content: [] } } })))).toBe(false);
 expect(traceWindowReady(rows.map((_, n) => traceRecord(n, { kind: n % 2 ? 'assistant' : 'user', location: { attempt_id: `turn-${Math.floor(n / 10)}` } })))).toBe(true);
 const contextOnly = Array.from({ length: 600 }, (_, n) => ({ ...entry(n + 1), item: { type: 'message' as const, message: { role: 'user' as const, id: `context-${n}`, source: 'runtime' as const, kind: { context: 'native_environment' as const }, content: [] } } }));
 expect(transcriptWindowReady(contextOnly)).toBe(false);
 expect(traceWindowReady(contextOnly.map((_, n) => traceRecord(n, { kind: 'tool' })))).toBe(false);
});
it('automatically reads a useful initial window and retains normal live updates without a historical-window mode', async () => {
 server = new Server(); const all = Array.from({ length: 120 }, (_, n) => entry(n + 1));
 server.snapshots.set('A', { ...snapshot(), transcript: { entries: all.slice(-4), next_cursor: '117' } }); await server.attached('A');
 server.handlers.set('session/transcript', request => {
  if (request.method !== 'session/transcript' || request.params.at.type !== 'older') throw Error('Expected bounded older read');
  const before = request.params.at.before;
  const rows = all.filter(row => BigInt(row.cursor) < BigInt(before)).slice(-request.params.limit);
  return { type: 'transcript_window', window: readingWindow({ entries: rows, next_cursor: rows[0]?.cursor === '1' ? null : rows[0]?.cursor }) };
 });
 await server.client.loadInitialHistory('A');
 const history = server.client.getSnapshot().views.A.history!;
 expect(history.window).toBeUndefined(); expect(history.loading).toBe(false);
 expect(history.page.entries).toHaveLength(68); expect(history.page.entries![0].cursor).toBe('53');
 expect(server.requests.filter(row => row.request.method === 'session/transcript')).toHaveLength(2);
 await server.client.loadInitialHistory('A');
 expect(server.requests.filter(row => row.request.method === 'session/transcript')).toHaveLength(2);
});
it('loads a short conversation to its beginning instead of stopping at a small transport page', async () => {
 server = new Server(); const all = Array.from({ length: 40 }, (_, n) => entry(n + 1));
 server.snapshots.set('A', { ...snapshot(), transcript: { entries: all.slice(-2), next_cursor: '39' } }); await server.attached('A');
 server.handlers.set('session/transcript', request => {
  if (request.method !== 'session/transcript' || request.params.at.type !== 'older') throw Error('Expected older read');
  const before = request.params.at.before;
  const rows = all.filter(row => BigInt(row.cursor) < BigInt(before)).slice(-request.params.limit);
  return { type: 'transcript_window', window: readingWindow({ entries: rows, next_cursor: rows[0]?.cursor === '1' ? null : rows[0]?.cursor }) };
 });
 await server.client.loadInitialHistory('A');
 expect(server.client.getSnapshot().views.A.history!.page.entries).toHaveLength(40);
 expect(server.client.getSnapshot().views.A.history!.page.next_cursor).toBeNull();
 const loaded = server.client.getSnapshot().views.A.history!.page.entries;
 server.client.returnToLatest('A'); await server.client.loadInitialHistory('A');
 expect(server.client.getSnapshot().views.A.history!.page.entries).toBe(loaded);
 expect(server.client.getSnapshot().views.A.history!.page.next_cursor).toBeNull();
});
