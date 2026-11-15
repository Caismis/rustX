import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFile,writeFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
import {normalized as normalizedPi} from './pi-oracle.mjs';

const TIME_PREFIX='<system-reminder>\nTimezone: UTC\nCurrent time: ';
const TIME_SUFFIX='\n</system-reminder>';
const CWD_MARKER='\n\nCurrent working directory: ';

function isPlainObject(value) {
 return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function parseEnvelopeArguments(raw) {
 const parsed=typeof raw==='string'?JSON.parse(raw):raw;
 assert(isPlainObject(parsed),'rust tool call arguments must decode to an object');
 assert(isPlainObject(parsed.arguments),'rust tool call must keep its invocation arguments envelope');
 return parsed;
}

export function rustWorkspaceOf(body) {
 let cwdWorkspace=null;
 let readWorkspace=null;
 for(const message of body.messages??[]) {
  if(message.role==='system' && typeof message.content==='string') {
   const idx=message.content.indexOf(CWD_MARKER);
   if(idx!==-1) {
    const rest=message.content.slice(idx+CWD_MARKER.length);
    const end=rest.indexOf('\n\n');
    cwdWorkspace=end===-1?rest:rest.slice(0,end);
   }
  }
  for(const call of message.tool_calls??[]) {
   const args=parseEnvelopeArguments(call.function.arguments);
   if(call.function.name==='read' && typeof args.arguments.path==='string') {
    assert(args.arguments.path.endsWith('/note.txt'),`unexpected read fixture path: ${args.arguments.path}`);
    const candidate=args.arguments.path.slice(0,-'/note.txt'.length);
    if(readWorkspace!==null) assert.equal(candidate,readWorkspace);
    readWorkspace=candidate;
   }
  }
 }
 if(cwdWorkspace!==null && readWorkspace!==null) assert.equal(readWorkspace,cwdWorkspace);
 return cwdWorkspace??readWorkspace??'/unused';
}

export function normalizedRustBody(body,dynamic=true) {
 const cloned=structuredClone(body);
 const workspace=dynamic?rustWorkspaceOf(cloned):'/unused';
 for(const message of cloned.messages??[]) {
  if(message.role==='system') {
   assert.equal(typeof message.content,'string');
   if(dynamic && workspace!=='/unused') {
    const exact=`${CWD_MARKER}${workspace}`;
    assert(message.content.includes(exact),'system prompt missing configured workspace cwd');
    message.content=message.content.replace(exact,`${CWD_MARKER}<WORKSPACE>`);
   }
   continue;
  }
  const content=Array.isArray(message.content)?message.content:[];
  for(const block of content) {
   if(dynamic && block.type==='text' && typeof block.text==='string' && block.text.startsWith(TIME_PREFIX)) {
    const stamp=block.text.slice(TIME_PREFIX.length,-TIME_SUFFIX.length);
    assert(block.text.endsWith(TIME_SUFFIX));
    assert(/^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d$/.test(stamp));
    assert.equal(new Date(stamp.replace(' ','T')+'Z').toISOString().slice(0,19).replace('T',' '),stamp);
    block.text=`${TIME_PREFIX}<TIME>${TIME_SUFFIX}`;
   }
  }
  for(const call of message.tool_calls??[]) {
   const args=parseEnvelopeArguments(call.function.arguments);
   if(dynamic && args.arguments.path===`${workspace}/note.txt`) args.arguments.path='<WORKSPACE>/note.txt';
   call.function.arguments=args;
  }
 }
 return cloned;
}

function piWorkspaceOf(wire) {
 for(const message of wire.body.messages??[]) for(const call of message.tool_calls??[]) {
  const args=JSON.parse(call.function.arguments);
  if(call.function.name==='read') return args.path.slice(0,-'/note.txt'.length);
 }
 return '/unused';
}

export function comparePayloads(rust,pi) {
 const sameEngine=pi.measurement_version !== undefined;
 const rightRequests=pi.wire_requests??pi.requests;
 assert.equal(rust.case,pi.scenario??pi.case);
 assert.equal(rust.wire_requests.length,rightRequests.length);
 const checked=[];
 for(let i=0;i<rightRequests.length;i++) {
  const r=rust.wire_requests[i],p=rightRequests[i];
  for(const key of ['method','path','protocol','model','index']) assert.deepEqual(r[key],p[key]);
  const a=normalizedRustBody(r.body,true);
  const b=sameEngine?normalizedRustBody(p.body,true):normalizedPi(p.body,piWorkspaceOf(p),true);
  // Compare the core independently, and retain the meaningful schema differences
  // for cross-engine comparisons; same-engine enforces full normalized equality.
  const {tools:rustTools,...rustCore}=a,{tools:piTools,...piCore}=b;
  assert.deepEqual(rustCore,piCore);
  if(sameEngine) assert.deepEqual(a,b);
  const digest=x=>createHash('sha256').update(JSON.stringify(x)).digest('hex');
  checked.push({index:i,coreEqual:true,fullRequestEqual:sameEngine || JSON.stringify(a)===JSON.stringify(b),rustToolNames:rustTools.map(t=>t.function.name),piToolNames:piTools.map(t=>t.function.name),rustToolsSha256:digest(rustTools),piToolsSha256:digest(piTools)});
 }
 return {case:rust.case,checked};
}

if(process.argv[1] && import.meta.url===pathToFileURL(process.argv[1]).href) {
 const [rustFile,piFile,output]=process.argv.slice(2);
 const rust=JSON.parse(await readFile(rustFile)),pi=JSON.parse(await readFile(piFile));
 const summary=comparePayloads(rust,pi);
 await writeFile(output,JSON.stringify(summary,null,2));
}
