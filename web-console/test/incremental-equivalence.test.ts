// @vitest-environment node
import {readFileSync} from 'node:fs';
import {expect,it} from 'vitest';
import type {Notification,RuntimeClientCheckpoint} from '../../protocol/app-server/v57';
import {checkpointSnapshot,foldRuntimeEvent,observationCheckpoint} from '../../protocol/app-server/projection';

it('T14 native Checkpoint(C) + contiguous captured events equals independent Checkpoint(D)',()=>{
  const capture=JSON.parse(readFileSync('test/fixtures/incremental-native.json','utf8')) as {
    initial:{result:{checkpoint:RuntimeClientCheckpoint}};
    expected:{result:{checkpoint:RuntimeClientCheckpoint}};
    events:Extract<Notification,{method:'session/event'}>[];
  };
  let state=checkpointSnapshot(capture.initial.result.checkpoint),cursor=BigInt(capture.initial.result.checkpoint.cursor);
  for(const record of capture.events){
    expect(BigInt(record.params.cursor)).toBe(cursor+1n);
    state=foldRuntimeEvent(state,record.params.event);cursor++;
  }
  expect(cursor).toBe(BigInt(capture.expected.result.checkpoint.cursor));
  // Include native cuts, collection revisions, release frontiers, interactions,
  // canonical commit descriptors and execution/audit settlement facts.
  // Undefined optional properties do not exist in the actual JSON record.
  // Every serialized identity, native cut, revision and settlement stays exact.
  const folded=JSON.parse(JSON.stringify(observationCheckpoint(state,String(cursor))));
  expect(folded).toStrictEqual(capture.expected.result.checkpoint);
});

it('T14 native canonical cuts fold before transcript refresh, including delayed callbacks and C before J',()=>{
  const capture=JSON.parse(readFileSync('test/fixtures/incremental-canonical-native.json','utf8')) as {
    cases:{name:string;initial:{result:{checkpoint:RuntimeClientCheckpoint}};expected:{result:{checkpoint:RuntimeClientCheckpoint}};events:Extract<Notification,{method:'session/event'}>[]}[];
  };
  expect(capture.cases).toHaveLength(4);
  for(const item of capture.cases){
    let state=checkpointSnapshot(item.initial.result.checkpoint),cursor=BigInt(item.initial.result.checkpoint.cursor);
    const facts=(event:import('../../protocol/app-server/v57').RuntimeClientEvent):import('../../protocol/app-server/v57').RuntimeClientEvent[]=>event.type==='projection_transition'?event.events.flatMap(facts):[event];
    for(const record of item.events){
      expect(BigInt(record.params.cursor),item.name).toBe(cursor+1n);
      expect(facts(record.params.event).some(fact=>fact.type==='read_domains_updated'),item.name).toBe(false);
      state=foldRuntimeEvent(state,record.params.event);cursor++;
    }
    expect(cursor,item.name).toBe(BigInt(item.expected.result.checkpoint.cursor));
    // Normalize only JavaScript's omitted undefined fields to actual JSON wire
    // shape; retain every native identity, cut, frontier, C/J fact and revision.
    const folded=JSON.parse(JSON.stringify(observationCheckpoint(state,String(cursor))));
    expect(folded,item.name).toStrictEqual(item.expected.result.checkpoint);
  }
});

it('T19 genuine expired LiveRef remains in native Replay and Applied reaches D without materialization',()=>{
  const capture=JSON.parse(readFileSync('test/fixtures/incremental-native.json','utf8')) as {
    initial:{result:{checkpoint:RuntimeClientCheckpoint}};
    expected:{result:{checkpoint:RuntimeClientCheckpoint}};
    events:Extract<Notification,{method:'session/event'}>[];
    expired_live_reference:{cursor:string;reference:import('../../protocol/app-server/v57').ContentRef};
  };
  const witness=capture.expired_live_reference;
  expect(witness).toBeDefined();expect(witness.reference.source.type).toBe('live_publication_prefix');
  expect(witness.reference.readable_end).toBeGreaterThan(0);
  const facts=(event:import('../../protocol/app-server/v57').RuntimeClientEvent):import('../../protocol/app-server/v57').RuntimeClientEvent[]=>event.type==='projection_transition'?event.events.flatMap(facts):[event];
  const original=capture.events.find(row=>row.params.cursor===witness.cursor);
  expect(original).toBeDefined();
  expect(facts(original!.params.event)).toContainEqual(expect.objectContaining({type:'block_content_advanced',content_ref:witness.reference}));
  // The Rust capture checked actual Host/SQLite expiry before subscribing C.
  // This consumer has no content reader: deferred bytes are irrelevant to Applied.
  let state=checkpointSnapshot(capture.initial.result.checkpoint),applied=BigInt(capture.initial.result.checkpoint.cursor);
  let settledAfterExpiry=false,canonicalAfterExpiry=false;
  for(const row of capture.events){
    expect(BigInt(row.params.cursor)).toBe(applied+1n);
    state=foldRuntimeEvent(state,row.params.event);applied++;
    if(applied>BigInt(witness.cursor))for(const event of facts(row.params.event)){
      if(event.type==='attempt_settled')settledAfterExpiry=true;
      if(event.type==='message_descriptor_committed'||event.type==='message_committed')canonicalAfterExpiry=true;
    }
  }
  expect(settledAfterExpiry).toBe(true);expect(canonicalAfterExpiry).toBe(true);
  expect(applied).toBe(BigInt(capture.expected.result.checkpoint.cursor));
  expect(JSON.parse(JSON.stringify(observationCheckpoint(state,String(applied))))).toStrictEqual(capture.expected.result.checkpoint);
});
