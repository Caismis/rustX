import { readFileSync,writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpus,totalmem } from 'node:os';
const hash=data=>createHash('sha256').update(data).digest('hex');
const root='/tmp/pr556-step-analysis';
const child='/tmp/rustx-pr556/web-console/scripts/benchmark-cad-native.mjs';
const runtimes=[{name:'original-O3',path:'/tmp/pr556-build-O3/build/Release'},{name:'diagnostic-O3',path:root+'/build/Release'}];
const fixtures=['/tmp/rustx-pr556/web-console/test/fixtures/geometry/lod-assembly.step','/tmp/rustx-pr556/web-console/test/fixtures/geometry/bracket-array.step','/tmp/pr556-curved-200.step','/tmp/pr556-large-bspline.step'];
const report={scope:'Diagnostic-only internal STEP parse wall-clock aggregates; no optimization performance claims. Fresh processes/instances, profiling API disabled, shared product policy lodTiers1, same inputs/final quality.',environment:{timestampUTC:new Date().toISOString(),cpu:cpus()[0].model,node:process.version,totalmem:totalmem()},runtimeAssets:runtimes.map(runtime=>({...runtime,assets:Object.fromEntries(['js','wasm'].map(type=>[type,hash(readFileSync(runtime.path+'/occt-import-js.'+type))]))})),buildEvidence:JSON.parse(readFileSync(root+'/build-evidence.json','utf8')),samples:[]};
const save=()=>writeFileSync(root+'/phase-samples.json',JSON.stringify(report,null,2)+'\n');
for(let run=0;run<3;run++)for(const fixture of fixtures)for(const runtime of (run%2?[...runtimes].reverse():runtimes)){
 const result=spawnSync(process.execPath,['--expose-gc',child,'--child','--runtime',runtime.path,'--fixture',fixture,'--tiers','1'],{encoding:'utf8',maxBuffer:32*1024*1024});
 writeFileSync(root+`/stderr-${run}-${fixture.split('/').at(-1)}-${runtime.name}.log`,result.stderr);
 if(result.status!==0)throw new Error(result.stderr||`Child exit ${result.status}`);
 const value=JSON.parse(result.stdout),markers=result.stderr.split('\n').filter(line=>line.startsWith('STEP_DIAGNOSTIC '));
 if(markers.length!==(runtime.name==='diagnostic-O3'?1:0))throw new Error('Expected exactly one diagnostic marker per instrumented read');
 report.samples.push({run,runtimeName:runtime.name,...value,internalStepProfile:markers.length?JSON.parse(markers[0].slice('STEP_DIAGNOSTIC '.length)):null});save();
 process.stderr.write(`run${run+1} ${runtime.name} ${fixture.split('/').at(-1)} read=${value.freshInstanceReadMs.toFixed(1)}ms\n`);
}
report.equivalence=[];
for(const fixture of fixtures){const samples=report.samples.filter(sample=>sample.fixture===fixture),fields=['sourceBytes','sourceSha256','finalDigest','finalNumericBitsDigest','finalMetadataDigest','meshes','triangles'];const result={fixture,checks:Object.fromEntries(fields.map(field=>[field,new Set(samples.map(sample=>sample[field])).size===1]))};report.equivalence.push(result);save();if(Object.values(result.checks).some(value=>!value))throw new Error('Strict output/source equivalence failed '+fixture);}
process.stderr.write('All strict source/final JSON/numeric-bits/metadata checks PASS\n');
