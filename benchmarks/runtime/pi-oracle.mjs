import assert from 'node:assert/strict';
const prefix='<system-reminder>\nTimezone: UTC\nCurrent time: ';
const suffix='\n</system-reminder>';
export const MODEL_TIME='2026-10-19 00:00:00';
const note='deterministic-note-payload-6d41';
export function normalized(body,workspace,dynamic=false) {
  body=structuredClone(body);
  for(const m of body.messages) {
    m.content=typeof m.content==='string'?[{type:'text',text:m.content}]:m.content??[];
    m.tool_calls??=null;
    for(const call of m.tool_calls??[]) {
      call.function.arguments=typeof call.function.arguments==='string'?JSON.parse(call.function.arguments):call.function.arguments;
      if(dynamic && call.function.arguments.path===workspace+'/note.txt') call.function.arguments.path='<WORKSPACE>/note.txt';
    }
  }
  return body;
}
const text=(role,value)=>({role,content:[{type:'text',text:value}],tool_calls:null});
export function expected(caseName,index,tools) {
  const messages=[text('user','benchmark turn 0000'),text('user',prefix+MODEL_TIME+suffix)];
  if(caseName.startsWith('short-')) for(let i=0;i<index;i++) messages.push(text('assistant',`reply ${String(i).padStart(4,'0')}`),text('user',`benchmark turn ${String(i+1).padStart(4,'0')}`));
  else if(caseName==='tool' && index===1) messages.push({role:'assistant',content:[],tool_calls:[{type:'function',id:'benchmark-read',function:{name:'read',arguments:{path:'<WORKSPACE>/note.txt'}}}]},{...text('tool',note),tool_call_id:'benchmark-read'});
  else assert.equal(index,0);
  return {model:'chat-model',messages,stream:true,stream_options:{include_usage:true},max_completion_tokens:1024,tools};
}
export function project(messages,tools) {
  const out=[];
  for(const m of messages) {
    if(m.role==='system') {
      assert.equal(m.content,'');
      // The native positional system entry declares tool definitions. Its
      // empty text has no transport message; the schemas are checked below.
      assert.deepEqual(m.toolsAdded.map(t=>({type:'function',function:{name:t.name,description:t.description,parameters:t.parameters}})),tools);
      continue;
    }
    if(m.role==='user') out.push(text('user',typeof m.content==='string'?m.content:m.content.map(b=>{assert.equal(b.type,'text');return b.text;}).join('')));
    else if(m.role==='assistant') {
      const content=[],calls=[];
      for(const b of m.content) {
        if(b.type==='text') content.push({type:'text',text:b.text});
        else {assert.equal(b.type,'toolCall');calls.push({type:'function',id:b.id,function:{name:b.name,arguments:b.arguments}});}
      }
      out.push({role:'assistant',content,tool_calls:calls.length?calls:null});
    } else {
      assert.equal(m.role,'toolResult');
      assert.equal(m.isError,false);
      out.push({role:'tool',tool_call_id:m.toolCallId,content:m.content,tool_calls:null});
    }
  }
  return {model:'chat-model',messages:out,stream:true,stream_options:{include_usage:true},max_completion_tokens:1024,tools};
}
export function verify(caseName,index,workspace,wire,capture,tools) {
  assert.deepEqual(normalized(wire,workspace),normalized(project(capture,tools),workspace));
  assert.deepEqual(normalized(wire,workspace,true),expected(caseName,index,tools));
}
