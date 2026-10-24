import assert from 'node:assert/strict';
import {it} from 'node:test';
import {replaceFromSnapshot} from '../src/presentation/projection.ts';
import {isAttemptActive} from '../src/presentation/state.ts';
import {workingStatus} from '../src/ui/components/status.ts';
import {deferredAttemptIdentities} from '../src/presentation/message-descriptors.ts';
import {snapshot} from './support/fixtures.ts';
import type {AttemptIdentitySource} from '../../protocol/app-server/v57.ts';

const source:AttemptIdentitySource={type:'journal',domain:'journal.1',field:{sequence:1,path:['attempt_id'],encoded_bytes:9000}};
it('historical incomplete Attempt retains positive native fields and original owner without claiming live work',()=>{
 const native=snapshot({attempt:{attempt_id:null,attempt_id_source:source,phase:{type:'incomplete'},turn:4,foreground:[],model:null,execution_settings:null}});
 const state=replaceFromSnapshot(native,'0');
 assert.equal(state.attempt?.attemptId,undefined);assert.deepEqual(state.attempt?.attemptSource,source);assert.equal(state.attempt?.turn,4);
 assert.equal(isAttemptActive(state),false);assert.equal(workingStatus(state),undefined);
 assert.deepEqual(deferredAttemptIdentities(state).map(field=>field.source),[source]);
});
it('current native running phase remains busy even when the display alias is deferred',()=>{
 const native=snapshot({attempt:{attempt_id:null,attempt_id_source:source,phase:{type:'running'},turn:1,foreground:[],model:null,execution_settings:null}});
 const state=replaceFromSnapshot(native,'0');assert.equal(isAttemptActive(state),true);assert.equal(state.attempt?.attemptId,undefined);
});
