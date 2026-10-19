import assert from 'node:assert/strict';
import {it} from 'node:test';
import {fixtures} from '../../protocol/app-server/fixtures.ts';
import {decodeProtocolMessage} from '../src/protocol/decoder.ts';

const trace=fixtures.find(value=>'method' in value&&value.method==='session/trace');
if(!trace||!('method' in trace)||trace.method!=='session/trace')throw new Error('Missing native Trace request fixture');
const target=trace.params.target;

it('the generated Root Trace boundary admits 128 interests and rejects the 129th',()=>{
 const request={...trace,params:{...trace.params,records:Array.from({length:128},()=> 'trace:7')}};
 assert.ok(decodeProtocolMessage(request));
 assert.equal(decodeProtocolMessage({...request,params:{...request.params,records:[...request.params.records,'trace:8']}}),undefined);
});

it('the generated Trace result requires an integer bounded progress count',()=>{
 const response=fixtures.find(value=>'result' in value&&value.result?.type==='trace');
 if(!response||!('result' in response)||response.result?.type!=='trace')throw new Error('Missing native Trace result fixture');
 for(const refreshed of [0,1,128])assert.ok(decodeProtocolMessage({...response,result:{...response.result,refreshed}}));
 for(const refreshed of [-1,129,1.5,null,'1'])assert.equal(decodeProtocolMessage({...response,result:{...response.result,refreshed}}),undefined);
 const {refreshed,...missing}=response.result;void refreshed;
 assert.equal(decodeProtocolMessage({...response,result:missing}),undefined);
});

it('checkpoint and Agent enrichment retain their generated eight-interest admission',()=>{
 const records=Array.from({length:8},()=> 'trace:7');
 const checkpoint={jsonrpc:'2.0',id:'checkpoint',method:'session/checkpoint',params:{target,trace_records:records}};
 assert.ok(decodeProtocolMessage(checkpoint));
 assert.equal(decodeProtocolMessage({...checkpoint,params:{...checkpoint.params,trace_records:[...records,'trace:8']}}),undefined);
 const agent=fixtures.find(value=>'method' in value&&value.method==='agent/trace');
 if(!agent||!('method' in agent)||agent.method!=='agent/trace')throw new Error('Missing native Agent Trace fixture');
 assert.ok(decodeProtocolMessage({...agent,params:{...agent.params,records}}));
 assert.equal(decodeProtocolMessage({...agent,params:{...agent.params,records:[...records,'trace:8']}}),undefined);
});
