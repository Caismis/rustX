import test from 'node:test';
import assert from 'node:assert/strict';
import {PiObserver} from './pi-observer.mjs';
const start={type:'message_start',message:{role:'assistant',content:[{type:'text',text:'a'}]}};
const delta={type:'message_update',usage:{},changes:[{type:'text_delta',contentIndex:0,delta:'b'}]};
test('folds useful ordered content and native run completion',async()=>{
 const o=new PiObserver(()=>42);o.consume(start);o.consume(delta);assert.equal(o.message.content[0].text,'ab');assert.equal(o.firstContentAt,42);
 o.consume({type:'message_end',entry:{id:1,model:[{role:'assistant',content:[{type:'text',text:'abc'}]}]}});
 const done=o.waitForRun(7);o.consume({type:'run_end',inputs:[7]});await done;assert.equal(o.entries[0].id,1);
});
test('rejects missing start and mutated visible prefix',()=>{
 assert.throws(()=>new PiObserver().consume(delta));const o=new PiObserver();o.consume(start);
 assert.throws(()=>o.consume({type:'message_end',entry:{id:1,model:[{role:'assistant',content:[{type:'text',text:'z'}]}]}}));
});
test('rejects duplicated/reordered entries and orphan tool updates',()=>{
 const o=new PiObserver();o.consume({type:'entry_appended',entry:{id:2}});
 for(const id of [2,1]) assert.throws(()=>o.consume({type:'entry_appended',entry:{id}}));
 assert.throws(()=>o.consume({type:'tool_execution_update',toolCallId:'missing',output:{append:'x'}}));
});
test('replacement snapshot replaces current display and validates order',()=>{
 const o=new PiObserver();o.consume(start);o.consume({type:'snapshot',entries:[{id:1}],tools:[]});assert.equal(o.message,null);
 assert.throws(()=>o.consume({type:'snapshot',entries:[{id:2},{id:1}],tools:[]}));
});

test('replacement snapshot preserves consumed history and completes the observed run',async()=>{
 const o=new PiObserver();o.consume({type:'entry_appended',entry:{id:1,value:'kept'}});
 o.consume({type:'run_start',inputs:[7]});const done=o.waitForRun(7);
 o.consume({type:'snapshot',entries:[{id:1,value:'kept'},{id:2}],tools:[]});await done;
 assert.throws(()=>o.consume({type:'snapshot',entries:[{id:1,value:'changed'},{id:2}],tools:[]}));
 assert.throws(()=>o.consume({type:'run_end',inputs:[7]}));
});
test('tool argument mutation is rejected and result entry is consumed exactly',()=>{
 const o=new PiObserver();o.consume({type:'message_start',message:{role:'assistant',content:[{type:'toolCall',id:'c',name:'read',arguments:{path:'/workspace/'}}]}});
 assert.throws(()=>o.consume({type:'message_end',entry:{id:1,model:[{role:'assistant',content:[{type:'toolCall',id:'c',name:'read',arguments:{path:'/different/'}}]}]}}));
 const result={id:2,model:[{role:'toolResult',toolCallId:'c',content:[{type:'text',text:'result'}]}]};
 o.consume({type:'tool_execution_end',toolCallId:'c',entry:result});assert.deepEqual(o.toolResults,[result]);
});
