// Pi Durable at its pinned revision, using the production HTTP/SSE adapter.
import assert from 'node:assert/strict';
import { verify } from './pi-oracle.mjs';
import { PiObserver } from './pi-observer.mjs';
const observerMode = process.env.RUSTX_BENCH_OBSERVER_MODE ?? 'native-workflow';
assert(['native-workflow','no-live-subscriber','live-subscriber'].includes(observerMode));
import { execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, readFile, readdir, stat, rm, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createInterface } from 'node:readline';
import { performance } from 'node:perf_hooks';
const [checkoutArg, scenario, output] = process.argv.slice(2);
const checkout = resolve(checkoutArg);
const revision = execFileSync('git', ['-C', checkout, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
assert.equal(revision, '90d863f7c1183b965f099855715d6e88c7b8a52c');
const load = (path) => import(pathToFileURL(join(checkout, path)).href);
const { createModels, createProvider } = await load('packages/ai/src/models.ts');
const { openAICompletionsApi } = await load('packages/ai/src/api/openai-completions.lazy.ts');
const { Harness, createRegistry, GenerationTask, hook, watchEvents } = await load('packages/durable/src/index.ts');
const { CodingTools } = await load('packages/durable/src/tools/index.ts');
const { NodeExecutionEnv } = await load('packages/durable/src/env/node.ts');
const { allEntries, textOf } = await load('packages/durable/test/chat-support.ts');
const { context } = await load('packages/durable/test/session-support.ts');
const { openNodeSqliteStorage } = await load('packages/durable/src/storage/sqlite/node.ts');
const started = performance.now();
const lifecycle = [];
const processSample = async () => Object.fromEntries(await Promise.all(['stat','io','smaps_rollup'].map(async name => [name, await readFile(`/proc/self/${name}`, 'utf8').catch(() => null)])));
const phase = async name => lifecycle.push({phase:name, elapsed_us:Math.round((performance.now()-started)*1000), process:await processSample()});
await phase('before_initialization');
const rootDir = await mkdtemp(join(dirname(resolve(output)), 'pi-'));
const workspace = join(rootDir, 'workspace');
await mkdir(workspace);
await writeFile(join(workspace, 'note.txt'), 'deterministic-note-payload-6d41');
console.log('RUSTX_BENCH_READY ' + JSON.stringify({workspace, scenario:`runtime_benchmark_${scenario}`}));
const lines = createInterface({input:process.stdin});
const ready = JSON.parse(await new Promise(resolve => lines.once('line', resolve)));
lines.close();
assert.equal(ready.host, '127.0.0.1');
assert.equal(ready.scenario, `runtime_benchmark_${scenario}`);
const baseUrl = `http://127.0.0.1:${ready.port}`;
const control = async (path, method='GET') => {
  const response = await fetch(baseUrl + '/__control/' + path, {method});
  assert(response.ok, await response.clone().text());
  return response.json();
};
await phase('after_provider_start');
const models = createModels();
models.setProvider(createProvider({id:'benchmark', name:'Local benchmark', baseUrl:baseUrl+'/v1',
  auth:{apiKey:{name:'Fixture',resolve:async()=>({auth:{apiKey:'conformance-secret'}})}},
  models:[{id:'chat-model', name:'chat-model', api:'openai-completions', provider:'benchmark', baseUrl:baseUrl+'/v1', reasoning:false, input:['text'], cost:{input:0,output:0,cacheRead:0,cacheWrite:0},contextWindow:128000,maxTokens:1024,compat:{supportsStore:false,maxTokensField:'max_completion_tokens'}}],
  api:openAICompletionsApi()}));
const registry = createRegistry();
registry.install(CodingTools);
const captures=[];
registry.install({name:'benchmark-observer',hooks:[hook(GenerationTask,{beforeRequest:async ({messages},api,ctx)=>{
  const view=await api.context(api.conversationId,ctx);
  assert.deepEqual(view.messages,messages);
  captures.push({messages:structuredClone(messages),cutoff:view.entries.at(-1).id});
}})]});
const expectedTools=CodingTools.tools.map(t=>({type:'function',function:{name:t.name,description:t.description,parameters:JSON.parse(JSON.stringify(t.parameters))}}));
const reports = [];
const options = {models,registry,settings:{stream:{maxRetries:0,cacheRetention:'none'}},env:()=>new NodeExecutionEnv({cwd:workspace}),onReport:e=>reports.push(String(e))};
const open = async () => {
  const harness = await Harness.open(await openNodeSqliteStorage(join(rootDir,'conversation.db')),options,context);
  const root = await harness.root(context,{agent:{model:{provider:'benchmark',modelId:'chat-model'},cwd:workspace}});
  return {harness,root};
};
let {harness,root} = await open();
harness.resume();
const observer = observerMode === 'live-subscriber' ? new PiObserver() : null;
const liveWatch = observer ? await watchEvents(harness,root.id,context) : null;
if(liveWatch) {
  observer.consume(liveWatch.snapshot);
  liveWatch.start(async events => { for(const event of events) observer.consume(event); });
}
await phase('after_initialization');
const history = scenario === 'short-small' ? 8 : scenario === 'short-large' ? 64 : 0;
const measured = history ? 32 : 1;
const samples = [];
const footprint = async () => Promise.all((await readdir(rootDir)).map(async name=>{const s=await stat(join(rootDir,name));return {name,logical:s.size,allocated:s.blocks*512};}));
const reply = i => scenario === 'large' ? 'L'.repeat(32768) : scenario === 'stream' ? Array.from({length:128},(_,n)=>`stream-delta-${String(n).padStart(4,'0')} `).join('') : scenario === 'tool' ? 'deterministic-note-payload-6d41' : `reply ${String(i).padStart(4,'0')}`;
for (let i=0;i<history+measured;i++) {
  const began = performance.now();
  if(observer) observer.firstContentAt=null;
  let content = `benchmark turn ${String(i).padStart(4,'0')}`;
  if (i===0) {
    // Persist the same two ordered input messages using Pi's native entry API.
    const input = await root.submit({type:'write',entry:{kind:'benchmark.input',model:[{role:'user',content,timestamp:Date.now()}]}},context);
    await input.wait(context);
    content = `<system-reminder>\nTimezone: UTC\nCurrent time: ${new Date().toISOString().slice(0,19).replace('T',' ')}\n</system-reminder>`;
  }
  const submission = await root.submit({type:'input',content},context);
  const accepted_us = Math.round((performance.now()-began)*1000);
  if (scenario==='cancel') {
    await control('observations/await?kind=gate_reached&name=cancel&timeoutMs=20000');
    await root.abort(context);
    await control('observations/await?kind=client_disconnected&timeoutMs=20000');
    await control('gates/cancel/release','POST');
  } else if (scenario==='stream') {
    for(let delta=0;delta<128;delta++) {
      await control(`observations/await?kind=gate_reached&name=delta-${delta}&timeoutMs=20000`);
      await new Promise(resolve=>setTimeout(resolve,10)); // identical provider workload pacing
      await control(`gates/delta-${delta}/release`,'POST');
    }
  }
  const settled = await submission.wait(context);
  assert.equal(settled.status, scenario==='cancel'?'unanswered':'done');
  if(observer) await Promise.race([
    observer.waitForRun(submission.id),
    liveWatch.closed.then(end=>{throw new Error(`observation ended before run: ${JSON.stringify(end)}`);}),
  ]);
  samples.push({observer_mode:observerMode,first_consumed_publication_us:observer?.firstContentAt == null ? null : Math.round((observer.firstContentAt-began)*1000),turn:i,measured:i>=history,accepted_us,settled_us:Math.round((performance.now()-began)*1000),process:await processSample(),footprint:await footprint()});
}
await phase('after_turns');
let lateSnapshot = null;
if(observerMode !== 'native-workflow') {
  const late = await watchEvents(harness,root.id,context);
  lateSnapshot = structuredClone(late.snapshot);
  assert.equal(lateSnapshot.run,undefined);
  assert.equal(lateSnapshot.generation,undefined);
  if(observer) {
    assert.deepEqual(observer.entries,lateSnapshot.entries);
    assert.equal(observer.tools.size,0);
    if(scenario==='stream') assert(observer.counts.message_update > 0);
    if(scenario==='tool') {
      assert.equal(observer.toolResults.length,1);
      const expected=lateSnapshot.entries.filter(entry=>entry.model?.some(message=>message.role==='toolResult'));
      assert.deepEqual(observer.toolResults,expected);
      assert(JSON.stringify(observer.toolResults[0]).includes('deterministic-note-payload-6d41'));
    } else assert.deepEqual(observer.toolResults,[]);
  }
  assert.equal((await late.stop()).reason,'stopped');
  if(liveWatch) assert.equal((await liveWatch.stop()).reason,'stopped');
  await phase('after_late_snapshot');
}
const requests = (await control('requests')).requests;
assert.equal(requests.length,history+measured+(scenario==='tool'?1:0));
assert.equal(captures.length,requests.length);
for(let i=0;i<requests.length;i++) verify(scenario,i,workspace,requests[i].body,captures[i].messages,expectedTools);
const entries = await allEntries(root);
if(lateSnapshot) assert.deepEqual(lateSnapshot.entries,entries);
const answers = entries.filter(e=>e.kind==='pi.assistant').flatMap(e=>e.model??[]).map(textOf).filter(v=>v!==undefined && v!=='');
assert.deepEqual(answers,scenario==='cancel'?[]:Array.from({length:history+measured},(_,i)=>reply(i)));
assert.deepEqual(reports,[]);
const liveContext = await root.context(context);
await phase('after_verification');
const before_close = await footprint();
await harness.close(context);
await phase('after_final_writer_close');
const closed_footprint = await footprint();
({harness,root}=await open());
assert.deepEqual(await allEntries(root),entries);
assert.deepEqual(await root.context(context),liveContext);
const reopenedRequests=[];
for(const capture of captures) {
  const view=await root.context(context,{at:capture.cutoff});
  assert.deepEqual(view.messages,capture.messages);
  reopenedRequests.push(view.messages);
}
for(let i=0;i<requests.length;i++) verify(scenario,i,workspace,requests[i].body,reopenedRequests[i],expectedTools);
await harness.close(context);
await phase('after_reopen_verification');
await rm(rootDir,{recursive:true});
await phase('after_cleanup');
await writeFile(output,JSON.stringify({observer_mode:observerMode,observer_surface:observerMode==='native-workflow'?'submission settlement':'Harness ConversationViews/watchEvents',observer_counts:observer?.counts??{},observed_partial_text:observer?.lastAssistantText??null,lateSnapshot,revision,scenario,history,measured,samples,lifecycle,requests,captures,reopenedRequests,expectedTools,entries,liveContext,before_close,closed_footprint,oracle:'passed',allocations:null,provider:'production openAICompletionsApi HTTP/SSE'},null,2));
