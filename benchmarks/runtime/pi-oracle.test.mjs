import test from 'node:test';
import assert from 'node:assert/strict';
import { expected, normalized, verify } from './pi-oracle.mjs';
const tools=[];
function fixture() {
 const body=expected('short-small',3,tools);
 body.messages[1].content[0].text=body.messages[1].content[0].text.replace('<TIME>','2026-10-18 00:00:00');
 const capture=body.messages.map(m=>({role:m.role,content:structuredClone(m.content)}));
 return {body,capture};
}
test('complete expected, wire and captured messages agree',()=>{const {body,capture}=fixture();verify('short-small',3,'/fixture',body,capture,tools);});
for(const [name,change] of [
 ['omitted old history',x=>x.splice(2,2)],
 ['reordered old history',x=>[x[2],x[3]]=[x[3],x[2]]],
 ['changed old history',x=>x[2].content[0].text='changed'],
 ['extra old history',x=>x.push(structuredClone(x[0]))],
]) test(name+' fails even when wire and capture agree',()=>{const {body,capture}=fixture();change(body.messages);change(capture);assert.throws(()=>verify('short-small',3,'/fixture',body,capture,tools));});
test('changed captured timestamp fails',()=>{const {body,capture}=fixture();capture[1].content[0].text=capture[1].content[0].text.replace('00:00:00','00:00:01');assert.throws(()=>verify('short-small',3,'/fixture',body,capture,tools));});
test('wrong tool schema fails',()=>{const {body,capture}=fixture();body.tools=[{type:'function',function:{name:'unexpected'}}];assert.throws(()=>verify('short-small',3,'/fixture',body,capture,tools));});
test('normalization preserves unknown semantic fields',()=>{const {body}=fixture();body.temperature=0.8;assert.notDeepEqual(normalized(body,'/fixture',true),expected('short-small',3,tools));});
