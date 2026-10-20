// Postprocessing only; reuse and hash the frozen published full-semantic oracle.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {resolve,dirname,join,basename} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const [oraclePath,toolsPath,output,...files]=process.argv.slice(2);
assert(oraclePath&&toolsPath&&output&&files.length);
const {normalized,expected,MODEL_TIME}=await import(pathToFileURL(resolve(oraclePath)));
const normativeTools=JSON.parse(await readFile(toolsPath,'utf8'));
const hash=b=>createHash('sha256').update(b).digest('hex');
const results=[],pairs=new Map();
for(const file of files){
  const bytes=await readFile(file),sample=JSON.parse(bytes),cohort=dirname(dirname(file));
  const manifest=JSON.parse(await readFile(join(cohort,'cohort-manifest.json'),'utf8'));
  const run=manifest.runs.find(r=>basename(r.argv?.at(-1)??'')===basename(dirname(file)));
  assert(run&&run.accepted_by_producer&&run.exit===0);
  assert.equal(sample.oracle,'passed');assert.equal(sample.model_time,MODEL_TIME);
  assert.equal(sample.complete.model_time,MODEL_TIME);
  const wire=sample.complete.wire_requests;
  assert.equal(wire.length,{'short-small':40,'short-large':96,'details-resync':2}[sample.case]);
  assert(sample.provider_report.ok);
  assert.deepEqual(wire,sample.provider_report.requests);
  const workspace=join(run.argv.at(-1),'fixture','workspace');
  const values=wire.map((request,index)=>{
    assert.equal(request.method,'POST');assert.equal(request.path,'/v1/chat/completions');
    const value=normalized(request.body,workspace,true);
    assert.deepEqual(value,expected(sample.case==='details-resync'?'tool':sample.case,index,normativeTools));
    return value;
  });
  const key=JSON.stringify([cohort,run.case,run.pair]);
  const entry=pairs.get(key)??{};assert(!entry[run.label]);entry[run.label]=values;pairs.set(key,entry);
  results.push({file,sha256:hash(bytes),source:sample.source,requests:wire.length,full_semantic_expected_equal:true});
}
let completePairs=0,pairedRequests=0;
for(const entry of pairs.values()){
  if(Object.keys(entry).length!==2)continue;
  const [a,b]=Object.values(entry);assert.deepEqual(a,b);completePairs++;pairedRequests+=a.length;
}
await writeFile(output,JSON.stringify({oracle_sha256:hash(await readFile(oraclePath)),expected_tools_sha256:hash(await readFile(toolsPath)),scope:'All native tools included using published pi-oracle normalized(body,workspace,dynamic=true), exact model time, exact recorded fixture workspace/note.txt substitution only. Text/content/null/tool-argument representation normalization, not literal HTTP-byte equality. This oracle has no sameEngine parameter. No Rust/Pi assertion. Failed products have no accepted complete/provider report and are excluded from this pass, with raw failures retained.',complete_pairs:completePairs,paired_requests:pairedRequests,results},null,2)+'\n');
console.log(JSON.stringify({products:results.length,requests:results.reduce((a,r)=>a+r.requests,0),completePairs,pairedRequests,output}));
