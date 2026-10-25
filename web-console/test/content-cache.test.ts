import { expect, it } from 'vitest';
import { ContentCache } from '../../protocol/app-server/content-cache';
import type { ContentChunk, ContentRef } from '../../protocol/app-server/v57';
import { contentRefKey, contentSourceKey, sameContentRef } from '../../protocol/app-server/content-identity';

const reference = (position: number, end = 0): ContentRef => ({
  conversation_id: 'c', source: {type: 'canonical_message_block', component: 'payload', locator: {type: 'by_ledger_position', position}, block_index: 0},
  representation: 'utf8_text', version: 1, content_revision: 1, readable_end: end,
});
const chunk = (ref: ContentRef): ContentChunk => ({content_ref: ref, offset: 0, end_offset: ref.readable_end, readable_end: ref.readable_end, data_base64: btoa('x'.repeat(ref.readable_end)), next_offset: null, complete: true});

it('T20 empty content ranges cannot bypass the finite locator workset', async () => {
  let physical = 0;
  const cache = new ContentCache(async ref => { physical++; return chunk(ref); });
  for (let position = 0; position < 65; position++) await cache.read(reference(position), 0, () => true);
  expect(physical).toBe(65);
  await cache.read(reference(64), 0, () => true); expect(physical).toBe(65);
  await cache.read(reference(0), 0, () => true); expect(physical).toBe(66);
});

it('T20 simultaneous exact-range completions retain one decoded-byte charge', async () => {
  let physical = 0;
  const gates: ((value: ContentChunk) => void)[] = [];
  const cache = new ContentCache(async () => { physical++; return new Promise(resolve => gates.push(resolve)); }, 4);
  const ref = reference(1, 4);
  const first = cache.read(ref, 0, () => true), second = cache.read(ref, 0, () => true);
  expect(gates).toHaveLength(2);
  gates[0]!(chunk(ref)); await first;
  gates[1]!(chunk(ref)); await second;
  expect(physical).toBe(2);
  expect(Array.from((await cache.read(ref, 0, () => true)).bytes)).toEqual([120, 120, 120, 120]);
  expect(physical).toBe(2);
});

it('T08/T11 exact ContentRef identity survives reordered native structural fields', async () => {
  const ref = reference(3, 4);
  const reordered: ContentRef = {readable_end:4,content_revision:1,version:1,representation:'utf8_text',
    source:{block_index:0,locator:{position:3,type:'by_ledger_position'},component:'payload',type:'canonical_message_block'},conversation_id:'c'};
  expect(sameContentRef(ref, reordered)).toBe(true);
  let physical = 0;
  const cache = new ContentCache(async () => { physical++; return chunk(reordered); });
  expect(new TextDecoder().decode((await cache.read(ref, 0, () => true)).bytes)).toBe('xxxx');
  await cache.read(reordered, 0, () => true);
  expect(physical).toBe(1);
});

it('T09/T12 each closed source and its read evidence remain distinct', () => {
  const canonical = reference(1, 4);
  const publication = {component:'payload' as const,stream_id:'s',message_id:'m',attempt_id:'a',turn_id:'t',publication_request_id:'r',block_index:0};
  const live: ContentRef = {...canonical,source:{type:'live_publication_prefix',...publication}};
  const audit: ContentRef = {...live,source:{type:'durable_transcript_audit',...publication}};
  expect(contentSourceKey(live)).not.toBe(contentSourceKey(audit));
  expect(contentSourceKey(canonical)).not.toBe(contentSourceKey(live));
  const reordered: ContentRef = {...live,source:{block_index:0,publication_request_id:'r',turn_id:'t',attempt_id:'a',message_id:'m',stream_id:'s',component:'payload',type:'live_publication_prefix'}};
  expect(sameContentRef(live,reordered)).toBe(true);
  for (const field of ['stream_id','message_id','attempt_id','turn_id','publication_request_id'] as const) {
    expect(sameContentRef(live,{...live,source:{...live.source,[field]:'changed'}})).toBe(false);
  }
  const variants: ContentRef[] = [
    {...live,conversation_id:'foreign'}, {...live,version:2}, {...live,representation:'raw_json_utf8'},
    {...live,content_revision:2}, {...live,readable_end:3},
    {...live,source:{type:'live_publication_prefix',...publication,block_index:1}},
    {...live,source:{type:'live_publication_prefix',...publication,component:'block_metadata'}},
  ];
  for(const value of variants)expect(contentRefKey(value)).not.toBe(contentRefKey(live));
  for(const locator of [{type:'by_ledger_position' as const,position:2},{type:'by_message_id' as const,message_id:'1'}]) {
    expect(sameContentRef(canonical,{...canonical,source:{type:'canonical_message_block',component:'payload',locator,block_index:0}})).toBe(false);
  }
  expect(contentSourceKey({...live,content_revision:2,readable_end:5})).toBe(contentSourceKey(live));
});
