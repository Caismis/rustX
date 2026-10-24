import { afterEach, expect, it } from 'vitest';
import type { RuntimeClientCompactionView } from '../../protocol/app-server/v57';
import { Server, snapshot } from './fixture';

let server: Server;
afterEach(() => server?.client.disconnect());
const receipt = (generation: string): RuntimeClientCompactionView => ({
  generation, checkpoint_id:`conv_8498648a-66ba-716f-8243-8cf147533e5${generation}`, activation_id:generation,
  tokens_before:{input_tokens:'8400',source:'estimated'}, estimated_tokens_after:'1900',
});

it.each([false,true])('exact compaction receipt is retained independently of newer published context (unavailable=%s)', async unavailable => {
  server = new Server();
  server.snapshots.set('A', {...snapshot(),context:{compaction_count:9,compaction_in_progress:false,latest_compaction:receipt('9')}});
  await server.attached('A');
  const target = server.target('A'), registration = server.registrations.get(JSON.stringify(target))!;
  if (unavailable) server.socket.deliver({jsonrpc:'2.0',method:'session/observationUnavailable',params:{target,registration_id:registration,message:'held original source'}});
  const before = server.client.getSnapshot().views.A;
  const reads = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
  const original = receipt('7');
  server.handlers.set('context/compact', () => ({type:'context_compacted',compaction:original}));
  server.held.add('context/compact');
  expect(server.client.compact('A')).toBe(true);
  const request = await server.waitFor('context/compact',1);
  const finished = new Promise<void>(resolve => {
    const stop = server.client.subscribe(() => {
      if (server.client.getSnapshot().views.A.compactionRequest?.status === 'succeeded') {stop();resolve();}
    });
  });
  server.reply(request); await finished;
  const after = server.client.getSnapshot().views.A;
  expect(after.compactionRequest?.receipt).toEqual(original);
  expect(after.snapshot).toBe(before.snapshot); expect(after.cursor).toBe(before.cursor);
  expect(after.snapshot?.context?.latest_compaction?.generation).toBe('9');
  expect(after.observationState).toBe(before.observationState);
  expect(server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(reads);
  expect(server.requests.filter(row => row.request.method === 'context/compact')).toHaveLength(1);
});
