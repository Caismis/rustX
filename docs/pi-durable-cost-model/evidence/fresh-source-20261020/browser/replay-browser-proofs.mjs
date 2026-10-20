// Postprocessing only: replay the frozen observer, without launching a browser.
import assert from 'node:assert/strict';
import {readFile,writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
const [drainPath,output,...samples]=process.argv.slice(2);
assert(drainPath&&output&&samples.length);
const {DrainTracker}=await import(pathToFileURL(resolve(drainPath)));
const hash=b=>createHash('sha256').update(b).digest('hex');
const results=[];
for(const file of samples){
  const bytes=await readFile(file),sample=JSON.parse(bytes),tracker=new DrainTracker();
  assert.equal(sample.oracle,'passed');
  const events=sample.drain_observations;let index=0;
  function consume(){
    const event=events[index++];assert(event);
    if(event.op==='open')tracker.open();
    else {assert.equal(event.op,'message');tracker.observe(event.message,event.direction,event.epoch);}
    assert.equal(tracker.generation,event.generation);
    assert.equal(tracker.epoch,event.epoch);
  }
  for(const turn of sample.samples){
    const proof=turn.full_repair_proof;
    while(tracker.generation<proof.generation)consume();
    assert.equal(tracker.generation,proof.generation);
    const replay=tracker.proof(new Map(proof.loaded_ids.map(id=>[id,true])));
    assert.deepEqual(replay,proof);
  }
  while(index<events.length)consume();
  results.push({file,sha256:hash(bytes),events:events.length,turn_proofs:sample.samples.length,logical_wire_proofs_replayed:true});
}
await writeFile(output,JSON.stringify({drain_sha256:hash(await readFile(drainPath)),scope:'Replays recorded logical wire proof exactly. Loaded-ID lists are producer observations. Does not independently establish rAF execution, route.send delivery, React queues, completed paints or offscreen rendering.',results},null,2)+'\n');
console.log(JSON.stringify({products:results.length,proofs:results.reduce((a,r)=>a+r.turn_proofs,0),output}));
