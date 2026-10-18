import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {createHash} from 'node:crypto';
import {normalized} from './pi-oracle.mjs';
const [rustFile,piFile,output]=process.argv.slice(2);
const rust=JSON.parse(await readFile(rustFile)),pi=JSON.parse(await readFile(piFile));
assert.equal(rust.case,pi.scenario);
assert.equal(rust.wire_requests.length,pi.requests.length);
const checked=[];
for(let i=0;i<pi.requests.length;i++) {
 const r=rust.wire_requests[i],p=pi.requests[i];
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
 const digest=x=>createHash('sha256').update(JSON.stringify(x)).digest('hex');
 checked.push({index:i,coreEqual:true,fullRequestEqual:JSON.stringify(a)===JSON.stringify(b),rustToolNames:rustTools.map(t=>t.function.name),piToolNames:piTools.map(t=>t.function.name),rustToolsSha256:digest(rustTools),piToolsSha256:digest(piTools)});
}
await writeFile(output,JSON.stringify({case:rust.case,checked},null,2));
