import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {normalized,MODEL_TIME} from './pi-oracle.mjs';
const [rustFile,piFile,output]=process.argv.slice(2);
const rust=JSON.parse(await readFile(rustFile)),pi=JSON.parse(await readFile(piFile));
for(const sample of [rust,pi]) { assert.equal(sample.workload_version,3); assert.equal(sample.model_time,MODEL_TIME); }
const modes=['native-workflow','no-live-subscriber','live-subscriber'];
const mode=rust.observer_mode??'native-workflow';
assert(modes.includes(mode));
if(mode!=='native-workflow' || pi.observer_mode!==undefined) assert.equal(pi.observer_mode,mode,'paired observer mode');
if(mode!=='native-workflow') {
 assert(rust.observer_mode && pi.observer_mode);
 for(const sample of [rust,pi]) assert(sample.samples.every(turn=>turn.observer_mode===mode));
}
const sameEngine=pi.measurement_version !== undefined;
const rightRequests=pi.wire_requests??pi.requests;
assert.equal(rust.case,pi.scenario??pi.case);
assert.equal(rust.wire_requests.length,rightRequests.length);
const checked=[];
for(let i=0;i<rightRequests.length;i++) {
 const r=rust.wire_requests[i],p=rightRequests[i];
 for(const key of ['method','path','protocol','model','index']) assert.deepEqual(r[key],p[key]);
 const workspaceOf=(wire)=>{
  for(const message of wire.body.messages) for(const call of message.tool_calls??[]) {
   const args=JSON.parse(call.function.arguments);
   if(call.function.name==='read') return args.path.slice(0,-'/note.txt'.length);
  }
  return '/unused';
 };
 const a=normalized(r.body,workspaceOf(r),true),b=normalized(p.body,workspaceOf(p),true);
 // Compare the core independently, and retain the meaningful schema differences.
 // This is explicitly not full request equality or feature/durability equivalence.
 const {tools:rustTools,...rustCore}=a,{tools:piTools,...piCore}=b;
 assert.deepEqual(rustCore,piCore);
 if(sameEngine) assert.deepEqual(a,b);
 const digest=x=>createHash('sha256').update(JSON.stringify(x)).digest('hex');
 checked.push({index:i,coreEqual:true,fullRequestEqual:sameEngine || JSON.stringify(a)===JSON.stringify(b),rustToolNames:rustTools.map(t=>t.function.name),piToolNames:piTools.map(t=>t.function.name),rustToolsSha256:digest(rustTools),piToolsSha256:digest(piTools)});
}
await writeFile(output,JSON.stringify({workload_version:3,model_time:MODEL_TIME,case:rust.case,observer_mode:mode,checked},null,2));
