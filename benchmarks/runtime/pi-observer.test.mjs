import test from 'node:test';
import assert from 'node:assert/strict';
import {PiObserver} from './pi-observer.mjs';
import {observerFixture,NOTE} from './pi-observer-fixture.mjs';
const text=t=>({role:'assistant',content:[{type:'text',text:t}]});
const entry=(id,t)=>({id,model:[text(t)]});
const observer=(expected='abc',now=()=>42)=>new PiObserver({now,assistant:()=>({content:text(expected).content})});
const snapshot=(entries=[],generation)=>({type:'snapshot',entries,tools:[],...(generation?{generation:{message:generation}}:{})});
const start={type:'message_start',message:text('a')};
const delta={type:'message_update',usage:{},changes:[{type:'text_delta',contentIndex:0,delta:'b'}]};
test('folds useful ordered content and native run completion',async()=>{
 const o=observer();o.consume(start);o.consume(delta);assert.equal(o.message.content[0].text,'ab');assert.equal(o.firstContentAt,42);
 o.consume({type:'message_end',entry:entry(1,'abc')});
 const done=o.waitForRun(7);o.consume({type:'run_end',inputs:[7]});await done;assert.equal(o.entries[0].id,1);
 assert(o.observedVersions.every(v=>v.valid));assert.equal(o.observedVersions.length,3);
});
test('rejects missing start and mutated visible text',()=>{
 assert.throws(()=>observer().consume(delta));const o=observer();o.consume(start);
 assert.throws(()=>o.consume({type:'message_end',entry:entry(1,'z')}));
});
test('rejects duplicated or reordered entries and orphan tool updates',()=>{
 for(const id of [2,1]) { const o=observer();o.consume({type:'entry_appended',entry:{id:2}});assert.throws(()=>o.consume({type:'entry_appended',entry:{id}})); }
 assert.throws(()=>observer().consume({type:'tool_execution_update',toolCallId:'missing',output:{append:'x'}}));
});
test('generic native replacement can replace entries without erasing checked versions',()=>{
 const o=observer();o.consume(start);const before=structuredClone(o.observedVersions);
 o.consume(snapshot([{id:1,value:'old'}]));assert.equal(o.message,null);assert.deepEqual(o.observedVersions,before);
 o.consume(snapshot([{id:1,value:'new'}]));assert.equal(o.entries[0].value,'new');
 assert.throws(()=>o.consume(snapshot([{id:2},{id:1}])));
});
test('snapshot completion and first explicit run end are distinct sightings',async()=>{
 const o=observer();o.consume({type:'run_start',inputs:[7]});const done=o.waitForRun(7);
 o.consume(snapshot());await done;o.consume({type:'run_end',inputs:[7]});
 assert.equal(o.completedRuns.size,1);assert.equal(o.explicitCompletedRuns.size,1);
 assert.throws(()=>o.consume({type:'run_end',inputs:[7]}));
});
test('independent fixture rejects WRONG before a correcting whole-message replacement',()=>{
 const o=observer('correct');assert.throws(()=>o.consume({type:'message_start',message:text('WRONG')}));
 const evidence=structuredClone(o.observedVersions);
 assert.throws(()=>o.consume({type:'message_update',changes:[{type:'message',message:text('correct')}]}));
 assert.throws(()=>o.consume({type:'message_end',entry:entry(1,'correct')}));
 assert.deepEqual(o.observedVersions,evidence);assert.equal(evidence[0].valid,false);
});
test('whole-message and block replacements are checked against the independent output',()=>{
 for(const change of [{type:'message',message:text('WRONG')},{type:'block',contentIndex:0,block:{type:'text',text:'WRONG'}}]) {
  const o=observer('correct');o.consume({type:'message_start',message:text('c')});
  assert.throws(()=>o.consume({type:'message_update',changes:[change]}));assert.equal(o.observedVersions.at(-1).valid,false);
 }
 const o=observer('correct');o.consume({type:'message_start',message:text('cor')});
 o.consume({type:'message_update',changes:[{type:'message',message:text('c')}]});
 o.consume({type:'message_end',entry:entry(1,'correct')});assert.equal(o.entries.length,1);
});
test('invalid in-flight snapshot cannot be hidden by a later clean resync',()=>{
 const o=observer('correct');assert.throws(()=>o.consume(snapshot([],text('WRONG'))));
 const evidence=structuredClone(o.observedVersions);assert.throws(()=>o.consume(snapshot([entry(1,'correct')])));
 assert.deepEqual(o.observedVersions,evidence);
});
test('valid partial evidence survives resync and unchanged old text does not reset first content',()=>{
 let now=10;const o=observer('correct',()=>now);o.consume({type:'message_start',message:text('cor')});
 o.consume(snapshot());assert.equal(o.lastAssistantText,'cor');assert.equal(o.observedVersions[0].blocks[0].bytes,3);
 o.firstContentAt=null;now=20;o.consume(snapshot([],text('co')));assert.equal(o.firstContentAt,null);
 o.consume({type:'message_end',entry:entry(1,'correct')});assert.equal(o.firstContentAt,20);
 o.firstContentAt=null;now=30;o.consume(snapshot([entry(1,'correct')]));assert.equal(o.firstContentAt,null);
});
test('final-only message_end text supplies first consumed content',()=>{
 const o=observer('correct',()=>73);o.consume({type:'message_end',entry:entry(1,'correct')});
 assert.equal(o.firstContentAt,73);assert.equal(o.lastAssistantText,'correct');
});
test('cancellation fixture rejects the forbidden tail and preserves valid partial resync evidence',()=>{
 const o=new PiObserver(observerFixture('cancel','/workspace'));o.consume({type:'message_start',message:text('part')});o.consume(snapshot());
 assert.equal(o.lastAssistantText,'part');assert.equal(o.observedVersions[0].valid,true);
 const bad=new PiObserver(observerFixture('cancel','/workspace'));
 assert.throws(()=>bad.consume({type:'message_start',message:text('partialmust not appear')}));
 const canonical=new PiObserver(observerFixture('cancel','/workspace'));
 assert.throws(()=>canonical.consume({type:'message_end',entry:entry(1,'partial')}));
});
test('tool argument mutation cannot be corrected away and tool result is independently checked',()=>{
 const o=new PiObserver(observerFixture('tool','/workspace'));
 o.consume({type:'message_start',message:{role:'assistant',content:[{type:'toolCall',id:'benchmark-read',name:'read',arguments:{path:'/workspace/'}}]}});
 assert.throws(()=>o.consume({type:'message_update',changes:[{type:'message',message:{role:'assistant',content:[{type:'toolCall',id:'benchmark-read',name:'read',arguments:{path:'/different/'}}]}}]}));
 const good=new PiObserver(observerFixture('tool','/workspace'));
 const result={id:2,model:[{role:'toolResult',toolCallId:'benchmark-read',toolName:'read',isError:false,content:[{type:'text',text:NOTE}]}]};
 good.consume({type:'tool_execution_end',toolCallId:'benchmark-read',entry:result});assert.deepEqual(good.toolResults,[result]);
});
test('transient tool output is checked before a later replacement',()=>{
 const o=new PiObserver(observerFixture('tool','/workspace'));o.consume({type:'tool_execution_start',toolCallId:'benchmark-read'});
 assert.throws(()=>o.consume({type:'tool_execution_update',toolCallId:'benchmark-read',output:{set:'WRONG'}}));
 assert.throws(()=>o.consume({type:'tool_execution_update',toolCallId:'benchmark-read',output:{set:NOTE}}));
});

test('native positional system publication is accepted and assistant versions stay checked',()=>{
 const o=observer('correct');
 o.consume({type:'message_start',message:{role:'system',content:'',toolsAdded:[]}});
 o.consume({type:'message_end',entry:{id:1,model:[{role:'system',content:'',toolsAdded:[]}]}});
 o.consume({type:'message_start',message:text('c')});
 o.consume({type:'message_end',entry:entry(2,'correct')});
 assert.equal(o.assistantOrdinal,1);assert.equal(o.observedVersions.length,2);
});

test('malformed assistant version leaves sticky invalid evidence',()=>{
 const o=observer('correct');assert.throws(()=>o.consume({type:'message_start',message:{role:'assistant',content:'WRONG'}}));
 assert.equal(o.observedVersions.length,1);assert.equal(o.observedVersions[0].valid,false);
 assert.match(o.observedVersions[0].error,/content must be blocks/);
 assert.throws(()=>o.consume(snapshot([entry(1,'correct')])));
});
