import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFile,writeFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';
import {MODEL_TIME, normalized as normalizedPi} from './pi-oracle.mjs';

const TIME_PREFIX='<system-reminder>\nTimezone: UTC\nCurrent time: ';
const TIME_SUFFIX='\n</system-reminder>';
const FIXED_REMINDER=`${TIME_PREFIX}${MODEL_TIME}${TIME_SUFFIX}`;
const CWD_MARKER='\n\nCurrent working directory: ';
const OBSERVER_MODES=['native-workflow','no-live-subscriber','live-subscriber'];

function isPlainObject(value) {
 return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function parseEnvelopeArguments(raw) {
 const parsed=typeof raw==='string'?JSON.parse(raw):raw;
 assert(isPlainObject(parsed),'rust tool call arguments must decode to an object');
 assert(isPlainObject(parsed.arguments),'rust tool call must keep its invocation arguments envelope');
 return parsed;
}

function requireKnownWorkspace(record, label) {
 assert.equal(typeof record.workspace,'string',`${label} missing immutable fixture workspace string`);
 assert(record.workspace.length>0,`${label} workspace must not be empty`);
 return record.workspace;
}

function assertMeasurementIdentity(rust, pi) {
 for (const sample of [rust, pi]) {
  assert.equal(sample.workload_version, 3, 'workload_version');
  assert.equal(sample.model_time, MODEL_TIME, 'model_time must be the fixed fixture clock');
 }
 const mode = rust.observer_mode ?? 'native-workflow';
 assert(OBSERVER_MODES.includes(mode), `observer mode ${mode} is not allowed`);
 if (mode !== 'native-workflow' || pi.observer_mode !== undefined) {
  assert.equal(pi.observer_mode, mode, 'paired observer mode');
 }
 if (mode !== 'native-workflow') {
  assert(rust.observer_mode && pi.observer_mode);
  for (const sample of [rust, pi]) {
   assert(Array.isArray(sample.samples), 'non-default observer mode requires per-turn samples');
   assert(sample.samples.every(turn => turn.observer_mode === mode), 'per-turn observer mode');
  }
 }
 return mode;
}

// `dynamic` rewrites only the declared workspace cwd and read path. The fixed
// benchmark clock is never rewritten to <TIME>.
export function normalizedRustBody(body,workspace,dynamic=true) {
 if(dynamic) {
  assert.equal(typeof workspace,'string','known fixture workspace string is required');
  assert(workspace.length>0,'known fixture workspace must not be empty');
 }
 const cloned=structuredClone(body);
 let systemMessages=0;
 let reminders=0;
 const expectedCwdLine=`${CWD_MARKER}${workspace}`;
 for(const message of cloned.messages??[]) {
  if(message.role==='system') {
   systemMessages+=1;
   assert.equal(typeof message.content,'string');
   if(dynamic) {
    const first=message.content.indexOf(expectedCwdLine);
    assert.notEqual(first,-1,`system prompt missing exact workspace cwd: ${workspace}`);
    assert.equal(message.content.lastIndexOf(expectedCwdLine),first,'duplicate workspace cwd line in system prompt');
    const after=first+expectedCwdLine.length;
    assert(after===message.content.length || message.content.startsWith('\n\n',after),
     `system prompt cwd line does not end at section boundary for ${workspace}`);
    message.content=message.content.slice(0,first)+`${CWD_MARKER}<WORKSPACE>`+message.content.slice(after);
   }
   continue;
  }
  const content=Array.isArray(message.content)?message.content:[];
  for(const block of content) {
   if(dynamic && block.type==='text' && typeof block.text==='string' && block.text.startsWith(TIME_PREFIX)) {
    assert.equal(block.text, FIXED_REMINDER, 'model-facing time must be the fixed fixture clock');
    reminders+=1;
   }
  }
  for(const call of message.tool_calls??[]) {
   const args=parseEnvelopeArguments(call.function.arguments);
   if(dynamic && call.function.name==='read') {
    assert.equal(args.arguments.path,`${workspace}/note.txt`,
     `read tool path must target declared fixture workspace ${workspace}`);
    args.arguments.path='<WORKSPACE>/note.txt';
   }
   call.function.arguments=args;
  }
 }
 if(dynamic) {
  assert.equal(systemMessages,1,'rust request must contain exactly one system message');
  assert.equal(reminders,1,'request must contain exactly one fixed-clock reminder');
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
 const observerMode=assertMeasurementIdentity(rust,pi);
 const sameEngine=pi.measurement_version !== undefined;
 const rightRequests=pi.wire_requests??pi.requests;
 assert.equal(rust.case,pi.scenario??pi.case);
 assert.equal(rust.wire_requests.length,rightRequests.length);
 const rustWorkspace=requireKnownWorkspace(rust,'left rust measurement');
 const piWorkspace=sameEngine?requireKnownWorkspace(pi,'right rust measurement'):null;
 const checked=[];
 for(let i=0;i<rightRequests.length;i++) {
  const r=rust.wire_requests[i],p=rightRequests[i];
  for(const key of ['method','path','protocol','model','index']) assert.deepEqual(r[key],p[key]);
  const a=normalizedRustBody(r.body,rustWorkspace,true);
  const b=sameEngine?normalizedRustBody(p.body,piWorkspace,true):normalizedPi(p.body,piWorkspaceOf(p),true);
  // Compare the core independently, and retain the meaningful schema differences
  // for cross-engine comparisons; same-engine enforces full normalized equality.
  const {tools:rustTools,...rustCore}=a,{tools:piTools,...piCore}=b;
  assert.deepEqual(rustCore,piCore);
  if(sameEngine) assert.deepEqual(a,b);
  const digest=x=>createHash('sha256').update(JSON.stringify(x)).digest('hex');
  checked.push({index:i,coreEqual:true,fullRequestEqual:sameEngine || JSON.stringify(a)===JSON.stringify(b),rustToolNames:rustTools.map(t=>t.function.name),piToolNames:piTools.map(t=>t.function.name),rustToolsSha256:digest(rustTools),piToolsSha256:digest(piTools)});
 }
 return {workload_version:3,model_time:MODEL_TIME,observer_mode:observerMode,case:rust.case,checked};
}

if(process.argv[1] && import.meta.url===pathToFileURL(process.argv[1]).href) {
 const [rustFile,piFile,output]=process.argv.slice(2);
 const rust=JSON.parse(await readFile(rustFile)),pi=JSON.parse(await readFile(piFile));
 const summary=comparePayloads(rust,pi);
 await writeFile(output,JSON.stringify(summary,null,2));
}
