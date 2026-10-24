import { afterEach, expect, it } from 'vitest';
import type { RuntimeClientMessageDescriptor, RuntimeClientTranscriptEntry, RuntimeClientTranscriptPage } from '../../protocol/app-server/v57';
import { deferredToolDescriptors } from '../src/bindings/deferred-tools';
import { AgentReading } from '../src/client/agent-reading';
import { extendTranscriptWindow, refreshTranscript, replaceTranscript } from '../src/client/transcript';
import { Server, readingWindow, snapshot } from './fixture';
const row = (n: number): RuntimeClientTranscriptEntry => ({ cursor: String(n), item: { type: 'message', message: { id: `m${n}`, role: 'assistant', content: [{ type: 'text', text: `Message ${n}` }] } } });
const descriptor = (n: number): RuntimeClientMessageDescriptor => ({ identity: `m${n}`, locator: { type: 'by_ledger_position', position: n }, role: 'assistant', kind: 'message', block_count: '33', tool_call_count: '0', tool_calls: [], collections: [{ domain: `messages.ledger:${n}.blocks`, revision: '1', loaded: '0', total: '33', next_offset: '0' }], content: [{ path: ['content','0','text'], deferred: true, content_ref: { conversation_id: 'conversation-A', source: { type: 'canonical_message_block', locator: { type: 'by_ledger_position', position: n }, component: 'payload', block_index: 0 }, representation: 'utf8_text', version: 1, content_revision: 1, readable_end: 3 } }] });
const page = (n: number): RuntimeClientTranscriptPage => ({ entries: [row(n - 1)], deferred_entries: [{ offset: String(n), identity: `m${n}`, value: descriptor(n), content: descriptor(n).content, deferred: true }], collections: descriptor(n).collections, next_cursor: null });
function check(page: RuntimeClientTranscriptPage, n: number) {
 const parsed = deferredToolDescriptors([], page, page.entries ?? [], false).find(item => item.identity === `m${n}`);
 expect(parsed).toEqual(descriptor(n));
 expect(page.collections).toContainEqual(descriptor(n).collections[0]);
 expect(page.deferred_entries?.find(item => item.offset === String(n))?.content).toEqual(descriptor(n).content);
}
let server: Server | undefined, reader: AgentReading | undefined;
afterEach(() => { reader?.dispose(); reader = undefined; server?.client.disconnect(); server = undefined; });
it.each(['initial','gesture','child'] as const)('%s history retains deferred positions when returning to live and after another observation', async mode => {
 server = new Server(); const live = { ...snapshot(), transcript: { entries: [row(89), row(90)], next_cursor: '89' } };
 server.snapshots.set('A', live); await server.attached('A');
 const historical = readingWindow(page(70));
 let pages=0;
 server.handlers.set(mode === 'child' ? 'agent/transcript' : 'session/transcript', () => ({ type: 'transcript_window', window: ++pages===1 ? {...historical,page:{...historical.page,next_cursor:'69'}} : readingWindow(page(38)) }));
 if (mode === 'child') {
  reader = new AgentReading(server.client, server.target('A'), 'worker', live.conversation_id, () => true);
  reader.observe(live); await reader.loadInitial();
 } else if (mode === 'initial') await server.client.loadInitialHistory('A');
 else await server.client.loadEarlier('A');
 const history = () => mode === 'child' ? reader!.getSnapshot().history! : server!.client.getSnapshot().views.A.history!;
 expect(history().window).toBeUndefined(); expect(history().page.entries?.some(item => item.cursor === '69')).toBe(true); check(history().page,70); check(history().page,38); expect(pages).toBe(2);
 const next = { ...live, transcript: { entries: [row(90),row(91)], next_cursor: '90' } };
 if (reader) reader.observe(next); else await server.update('A', next);
 check(history().page,70); check(history().page,38); expect(pages).toBe(2);
});
it('multi-page aggregation keeps each descriptor, native binding and frontier, and inline promotion replaces it', () => {
 let cache = replaceTranscript({ entries: [row(89)], next_cursor: '89' });
 cache = extendTranscriptWindow(readingWindow({ ...page(70), next_cursor: '69' }),cache,true);
 cache = extendTranscriptWindow(readingWindow(page(38)),cache,true);
 const {window: _window, ...ordinary} = cache;
 cache = refreshTranscript(ordinary,{entries:[row(89),row(90)]});
 check(cache.page,70); check(cache.page,38);
 cache = refreshTranscript(cache,{entries:[row(70),row(90)]});
 expect(cache.page.deferred_entries?.map(item=>item.offset)).toEqual(['38']);
 expect(cache.page.entries?.filter(item=>item.cursor==='70')).toEqual([row(70)]);
 // A later byte-bounded head may represent the same position with a descriptor.
 cache = refreshTranscript(cache,{...page(70),entries:[row(90)]});
 expect(cache.page.entries?.some(item=>item.cursor==='70')).toBe(false);check(cache.page,70);
});
it('a deferred-to-inline overlap proves continuity without an inline shared anchor', () => {
 const prior=replaceTranscript(page(70));const next=refreshTranscript(prior,{entries:[row(70),row(71)]});
 expect(next.epoch).toBe(prior.epoch);expect(next.page.entries?.map(item=>item.cursor)).toEqual(['69','70','71']);
 expect(next.page.deferred_entries).toEqual([]);
});
