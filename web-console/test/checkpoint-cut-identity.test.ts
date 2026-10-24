import { expect, it } from 'vitest';
import type { CheckpointReadCut } from '../../protocol/app-server/v57';
import { checkpointReadCutKey, observationReadCutKey } from '../../protocol/app-server/content-identity';
import { readTraceCollection } from '../src/client/trace-content';
import { readCut } from './fixture';

const cut: CheckpointReadCut = {
  conversation_id: 'conversation-A', activation_id: 'activation-A', revision: '0', through: '0',
  source_cut: { read: readCut(), ledger_through: '0', transcript_total: '0', surface_revision: '0', inherited_through: '0' },
  domain_revisions: { z: '7', a: '1' }, collection_cuts: { z: { generation: '2', total: 4 }, a: { generation: '3', total: 1 } },
};
const wire: CheckpointReadCut = {
  canonical_foreground: null, live_publication: null, collection_cuts: { a: { total: 1, generation: '3' }, z: { total: 4, generation: '2' } },
  domain_revisions: { a: '1', z: '7' }, source_cut: { inherited_through: '0', surface_revision: '0', transcript_total: '0', ledger_through: '0',
    read: { pending_total: '0', pending_through: '0', transcript_total: '0', surface_revision: '0', ledger_through: '0', mutation_revision: '0', transcript: '0', journal: '0', conversation_id: 'conversation-A' } },
  through: '0', revision: '0', activation_id: 'activation-A', conversation_id: 'conversation-A',
};
it('native null selectors and reordered JSON fields retain the exact read identity', () => {
  expect(JSON.stringify(wire)).not.toBe(JSON.stringify(cut));
  expect(checkpointReadCutKey(wire)).toBe(checkpointReadCutKey(cut));
  expect(observationReadCutKey(wire.source_cut!)).toBe(observationReadCutKey(cut.source_cut!));
});
it('changes to the native owner, frontier or collection generation remain fenced', () => {
  const source = cut.source_cut!;
  for (const changed of [
    { ...cut, conversation_id: 'foreign' }, { ...cut, activation_id: 'foreign' }, { ...cut, revision: '1' }, { ...cut, through: '1' },
    { ...cut, source_cut: { ...source, read: { ...source.read, mutation_revision: '1' } } },
    { ...cut, source_cut: { ...source, read: { ...source.read, pending_through: '1' } } },
    { ...cut, source_cut: { ...source, inherited_through: '1' } },
    { ...cut, domain_revisions: { ...cut.domain_revisions, a: '2' } },
    { ...cut, collection_cuts: { ...cut.collection_cuts, a: { generation: '4', total: 1 } } },
    { ...cut, collection_cuts: { ...cut.collection_cuts, a: { generation: '3', total: 2 } } },
    { ...cut, canonical_foreground: { locator: { type: 'by_message_id' as const, message_id: 'message-A' }, total: 1 } },
    { ...cut, live_publication: { source: { type: 'live_publication_prefix' as const, component: 'payload' as const, block_index: 0,
      stream_id: 's', message_id: 'm', attempt_id: 'a', turn_id: 't', publication_request_id: 'r' }, revision: 1, block_total: 1, tool_total: 0 } },
  ]) expect(checkpointReadCutKey(changed)).not.toBe(checkpointReadCutKey(cut));
});
it('a native collection accepts semantically identical wire cuts across pages and rejects an owner change', async () => {
  let changed = false;
  const reader = { page: async (_cut: CheckpointReadCut, domain: string, offset: number) => ({
    read_cut: offset === 0 ? cut : changed ? { ...wire, activation_id: 'foreign' } : wire,
    cursor: '0', domain, total: '2', collections: [], next_offset: offset === 0 ? '1' : null,
    items: [{ offset: String(offset), identity: 'text', value: { type: 'text', text: `Paragraph ${offset}` }, content: [], deferred: false }],
  }) };
  expect(await readTraceCollection(reader, cut, { domain: 'messages.ledger:1.blocks', total: '2' }, () => true)).toHaveLength(2);
  changed = true;
  await expect(readTraceCollection(reader, cut, { domain: 'messages.ledger:1.blocks', total: '2' }, () => true)).rejects.toThrow('Original Trace content page changed.');
});
