import { expect, it } from 'vitest';
import type { AttemptIdentitySource, RuntimeClientTranscriptEntry, TurnProcessView } from '../../protocol/app-server/v57';
import { attemptIdentitySourceKey, attemptOwnerKey } from '../../protocol/app-server/attempt-identity';
import { turnProcesses } from '../src/bindings/turn-process';
import { turnPresentation } from '../src/bindings/turn-presentation';
import { turnAnchor } from '../src/client/transcript';

const source:AttemptIdentitySource={type:'journal',domain:'journal.41',field:{sequence:41,path:['attempt_id'],encoded_bytes:9000}};
const process=(attempt_id_source:AttemptIdentitySource):TurnProcessView=>({conversation_id:'original-owner',attempt_id:null,attempt_id_source,
  final_message_id:null,final_message_locator:null,final_message_id_deferred:false,final_message_id_source:null,
  outcome:'incomplete_at_cut',control_cursor:'1',message_count:2,tool_call_count:0});
const row=(cursor:string,owner:TurnProcessView):RuntimeClientTranscriptEntry=>({cursor,turn_process:owner,item:{type:'message',message:{role:'assistant',id:`member-${cursor}`,content:[{type:'reasoning',text:'original member'}]}}});

it('T14 shared selected owner identity is closed, stable under property reordering, and never aliases null owners',()=>{
  const key=attemptOwnerKey(process(source));
  const reordered:AttemptIdentitySource={field:{encoded_bytes:9000,path:['attempt_id'],sequence:41},domain:'journal.41',type:'journal'};
  expect(attemptOwnerKey(process(reordered))).toBe(key);
  expect(attemptIdentitySourceKey(reordered)).toBe(attemptIdentitySourceKey(source));
  expect(attemptOwnerKey({...process(source),conversation_id:'other-original-owner'})).not.toBe(key);
  expect(attemptOwnerKey(process({...source,field:{...source.field,sequence:42},domain:'journal.42'}))).not.toBe(key);
  expect(attemptOwnerKey(process({type:'inherited',kind:'turn',ordinal:0,domain:'transcript.turns.0.identity',encoded_bytes:9000}))).not.toBe(key);
  expect(()=>attemptOwnerKey({conversation_id:'original-owner',attempt_id:null,attempt_id_source:null})).toThrow('Native attempt owner identity is unavailable');
  expect(attemptOwnerKey({conversation_id:'original-owner',attempt_id:'exact-inline',attempt_id_source:null})).toBe(JSON.stringify(['original-owner','exact-inline']));
});

it('T14 different members and pagination share their genuine deferred native owner without using member cursors',()=>{
  const owner=process(source),first=row('1',owner),later=row('2',owner),other=row('3',process({type:'inherited',kind:'response',ordinal:0,domain:'transcript.responses.0.identity',encoded_bytes:9000}));
  const original=JSON.stringify([first,later,other]),small=turnProcesses([later]),large=turnProcesses([first,later,other]);
  // Incomplete processes are not folded, but retain exact membership ownership.
  const controls=turnPresentation([first,later,other]).filter(node=>node.kind==='process');
  expect(controls).toHaveLength(2);
  expect(controls.map(node=>node.kind==='process'?node.key:undefined)).toEqual([attemptOwnerKey(owner),attemptOwnerKey(other.turn_process!)]);
  expect(turnAnchor(later.turn_process!)).toBe(turnAnchor(first.turn_process!));
  expect(small.attempts.get(attemptOwnerKey(owner))).toBe(large.attempts.get(attemptOwnerKey(owner)));
  expect(JSON.stringify([first,later,other])).toBe(original);
});
