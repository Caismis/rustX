// Real browser + normal optimized native App Server, sibling scripted HTTP provider.
import assert from 'node:assert/strict';
import {spawn,execFileSync} from 'node:child_process';
import {createInterface} from 'node:readline';
import {readFile,writeFile,mkdir,rm,readdir,stat} from 'node:fs/promises';
import {resolve,join,dirname} from 'node:path';
import {pathToFileURL} from 'node:url';
import {createHash} from 'node:crypto';
import {performance} from 'node:perf_hooks';
import {DrainTracker} from './drain.mjs';
import {Diagnostics,installBrowserDiagnostics} from './diagnostics.mjs';
const diagnostics=new Diagnostics();
const [checkoutArg,binaryArg,caseName,outputArg]=process.argv.slice(2);
assert(['short-small','short-large','details-resync'].includes(caseName));
const checkout=resolve(checkoutArg),binary=resolve(binaryArg),output=resolve(outputArg);
await mkdir(output,{recursive:false});
const code=dirname(new URL(import.meta.url).pathname),fixture=join(output,'fixture');await mkdir(fixture);
const hostCase=caseName==='details-resync'?'tool':caseName;
const {chromium,expect}=await import(pathToFileURL(join(checkout,'web-console/node_modules/@playwright/test/index.mjs')));
const {startWorkspaceHost,routeWorkspaceHost}=await import(pathToFileURL(join(checkout,'web-console/test/e2e/workspace-host.ts')));
const {connectRemote,connectionAction}=await import(pathToFileURL(join(checkout,'web-console/test/e2e/shell-actions.ts')));
const {normalized,expected,MODEL_TIME}=await import(pathToFileURL(join(checkout,'benchmarks/runtime/pi-oracle.mjs')));
const normativeTools=JSON.parse(await readFile(join(code,'expected_tools.json'),'utf8'));
const env=Object.fromEntries(Object.entries(process.env).filter(([key])=>!/(TOKEN|SECRET|CREDENTIAL|API_KEY|PASSWORD)/i.test(key)));
env.TZ='UTC';env.UV_CACHE_DIR='/workspace/.uv-cache';
const records=[],failures=[],samples=[],memory=[],wire=[],requests=new Map(),traceRecords=new Map();
const completedAttempts=new Map();const drain=new DrainTracker();
let native,provider,browser,context,page,workspaceHost,preview,container,ready,report,complete;
let phase='setup',firstContent,settlements=[],holdDetail=false,held,activeAttempt,streamingText=new Map();
let browserUpstream,fixtureSequence=0,browserCdp,finishHeartbeat=false,heartbeat;
const fixturePending=new Map(),pressure=[];
const reply=i=>'reply '+String(i).padStart(4,'0');
const stream=Array.from({length:128},(_,i)=>`inspector-delta-${String(i).padStart(4,'0')} `).join('');
const errors=[];const began=performance.now();
const hash=bytes=>createHash('sha256').update(bytes).digest('hex');
const fixture_source={};const sourceCopy=join(output,'fixture-source');await mkdir(sourceCopy);
for(const name of (await readdir(code)).filter(n=>/\.(mjs|py|rs|json|txt)$/.test(n))){const bytes=await readFile(join(code,name));fixture_source[name]=hash(bytes);await writeFile(join(sourceCopy,name),bytes);}
const timer=promise=>Promise.race([promise,new Promise((_,reject)=>{const h=setTimeout(()=>reject(Error('fixture rendezvous deadline exhausted')),120000);h.unref();})]);
function child(argv,cwd=checkout,role='child'){const p=spawn(argv[0],argv.slice(1),{cwd,env,stdio:['pipe','pipe','pipe']});diagnostics.child(role,p);p.lines=[];p.errors='';p.stderr.on('data',s=>{p.errors=(p.errors+String(s)).slice(-65536);});const rl=createInterface({input:p.stdout});rl.on('line',line=>{p.lines.push(line);for(const w of [...(p.waiters??[])])if(w.test(line)){p.waiters.splice(p.waiters.indexOf(w),1);w.resolve(line);}});p.waiters=[];p.closed=new Promise(resolve=>p.once('exit',(code,signal)=>resolve({code,signal})));return p;}
async function line(p,test){const found=p.lines.find(test);if(found)return found;return timer(new Promise((resolve,reject)=>{p.waiters.push({test,resolve});p.once('exit',code=>reject(Error(`fixture exited before rendezvous:${code}\n${p.errors}`)));}));}
async function control(path,method='GET'){const r=await fetch(`http://${ready.host}:${ready.port}/__control/${path}`,{method});assert(r.ok,await r.clone().text());return r.json();}
async function counters(pid){const result={pid};for(const name of ['stat','io','smaps_rollup'])result[name]=await readFile(`/proc/${pid}/${name}`,'utf8').catch(()=>null);return result;}
async function browserCounters(){
  const script=`const fs=require('fs');const result=[];for(const pid of fs.readdirSync('/proc').filter(x=>/^\\d+$/.test(x))){let comm;try{comm=fs.readFileSync('/proc/'+pid+'/comm','utf8').trim()}catch{continue}if(!/chrome|headless/i.test(comm))continue;const row={pid:Number(pid),comm};for(const name of ['stat','io','smaps_rollup']){try{row[name]=fs.readFileSync('/proc/'+pid+'/'+name,'utf8')}catch{row[name]=null}}result.push(row)}process.stdout.write(JSON.stringify(result));`;
  return JSON.parse(execFileSync('docker',['exec',container,'node','-e',script],{encoding:'utf8'}));
}
async function browserCpu(){return (await browserCdp.send('SystemInfo.getProcessInfo')).processInfo;}
async function browserRead(method,params){const id=`fixture-read:${++fixtureSequence}`;return timer(new Promise((resolve,reject)=>{fixturePending.set(id,{resolve,reject});browserUpstream.send(JSON.stringify({jsonrpc:'2.0',id,method,params}));}));}
async function memoryPoint(name){memory.push({phase:name,elapsed_ms:performance.now()-began,native:await counters(native.pid),browser:await browserCounters(),js:await cdp.send('Runtime.getHeapUsage'),dom:await cdp.send('Memory.getDOMCounters')});}
function rows(){return page.locator('[data-request-owner]');}
function facts(e){return e.type==='projection_transition'?e.events:[e];}
function completed(id,outcome){assert.equal(outcome.type,'completed');const prior=completedAttempts.get(id);if(prior)assert.deepEqual(prior,outcome);else completedAttempts.set(id,outcome);}
function observedText(id,index,text){
  const key=id+':'+index,prior=streamingText.get(key)??'';
  assert(text.startsWith(prior)||prior.startsWith(text),'replacement changed an already visible prefix');
  const whole=caseName==='details-resync'?stream:reply(completedAttempts.size);
  // Historical complete messages have their own fixture ordinal; in-flight
  // replacement prefixes belong to the current attempted turn.
  assert(whole.startsWith(text),'replacement differs from the independent fixture output');
  if(text.length>=prior.length)streamingText.set(key,text);
}
function checkpointObserved(checkpoint){
  const inflight=checkpoint.snapshot.attempt?.in_flight;
  if(inflight)for(const [ordinal,block] of inflight.blocks.entries())if(block.type==='text'){
    const path=['attempt','in_flight','blocks',String(ordinal),'text'];
    assert(!checkpoint.content.some(binding=>JSON.stringify(binding.path)===JSON.stringify(path)&&binding.deferred),'small inspector checkpoint text must be complete inline');
    observedText(inflight.message_id,block.block_index,block.text);
  }
  const attempt=checkpoint.snapshot.attempt;
  if(attempt?.phase.type==='settled')completed(attempt.attempt_id,attempt.phase.outcome);
}
function observe(message,direction,transport){
  drain.observe(message,direction,transport);
  const at=performance.now();
  if(direction==='out'&&message.id){requests.set(message.id,message);records.push({at_ms:at,phase,transport_epoch:transport,direction,method:message.method,id:message.id,params:message.params});return;}
  if(direction!=='in')return;
  if(message.error){records.push({at_ms:at,phase,transport_epoch:transport,direction,id:message.id,error:message.error});return;}
  const result=message.result;
  if(result){
    const request=requests.get(message.id);
    records.push({at_ms:at,phase,transport_epoch:transport,direction,id:message.id,method:request?.method,result});
    const head=result.type==='trace'?result.page:result.type==='checkpoint'?result.checkpoint.snapshot.trace:undefined;
    if(result.type==='checkpoint')checkpointObserved(result.checkpoint);
    if(head)for(const record of [...head.records,...head.updates??[]])traceRecords.set(record.id,{...traceRecords.get(record.id),...record});
    return;
  }
  if(message.method==='session/resyncRequired')records.push({at_ms:at,phase,transport_epoch:transport,direction,notification:message});
  if(message.method!=='session/event')return;
  records.push({at_ms:at,phase,transport_epoch:transport,direction,notification:message});
  for(const fact of facts(message.params.event)){
    if(fact.type==='read_domains_updated'&&fact.trace)for(const record of [...fact.trace.records,...fact.trace.updates??[]])traceRecords.set(record.id,{...traceRecords.get(record.id),...record});
    if(fact.type==='checkpoint_advanced'){checkpointObserved(fact.checkpoint);for(const record of fact.checkpoint.snapshot.trace.records)traceRecords.set(record.id,{...traceRecords.get(record.id),...record});}
    if(fact.type==='attempt_started')activeAttempt=fact.attempt_id;
    if(fact.type==='attempt_settled'){
      assert(!settlements.some(item=>item.attempt_id===fact.attempt_id),'duplicate explicit terminal');
      completed(fact.attempt_id,fact.outcome);settlements.push(fact);
    }
    if(fact.type==='block_content_advanced'&&fact.kind==='text'){
      const key=fact.message_id+':'+fact.block_index,prior=streamingText.get(key)??'';
      assert.equal(Buffer.byteLength(prior),Number(fact.base_length));assert.equal(typeof fact.inline_delta,'string');
      const next=prior+fact.inline_delta;assert.equal(Buffer.byteLength(next),Number(fact.end_length));
      const whole=caseName==='details-resync'?stream:reply(completedAttempts.size);
      assert(whole.startsWith(next),'visible output changed its independent expected prefix');streamingText.set(key,next);
      firstContent??=at;
    }
  }
}
class Rpc{
  seq=0;pending=new Map();
  async open(endpoint,token,version){this.socket=new WebSocket(endpoint,[`rustx.app-server.v${version}`,`rustx-token.${token}`]);await timer(new Promise((resolve,reject)=>{this.socket.addEventListener('open',resolve,{once:true});this.socket.addEventListener('error',reject,{once:true});}));this.socket.addEventListener('message',e=>{const m=JSON.parse(e.data);if(m.id){const p=this.pending.get(m.id);this.pending.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result);}});await this.call('initialize',{protocol_version:version,client:{name:'active-inspector-controller',version:'1'},presentation:{images:false,questionnaires:true,reviews:true}});}
  async call(method,params){const id=`control:${++this.seq}`;const result=await timer(new Promise((resolve,reject)=>{this.pending.set(id,{resolve,reject});this.socket.send(JSON.stringify({jsonrpc:'2.0',id,method,params}));}));if(phase==='resync_pressure')pressure.push({method,params,result});return result;}
  async close(){if(!this.socket||this.socket.readyState===WebSocket.CLOSED)return;await new Promise(resolve=>{this.socket.addEventListener('close',resolve,{once:true});this.socket.close();});}
}
const rpc=new Rpc();let cdp;
try{
  native=child([binary,fixture,hostCase],checkout,'native');
  const needs=JSON.parse(await line(native,s=>{try{return JSON.parse(s).kind==='provider_needed';}catch{return false;}}));
  const providerArgs=caseName==='details-resync'
    ?['uv','run','--project',join(checkout,'test-support/fake-provider'),'--frozen','python',join(code,'provider.py')]
    :['uv','run','--project',join(checkout,'test-support/fake-provider'),'--frozen','fake-provider'];
  provider=child([...providerArgs,'--scenario',needs.scenario,'--workspace',needs.workspace,'--port','0'],checkout,'provider-command');
  ready=JSON.parse(await line(provider,s=>{try{return JSON.parse(s).ready===true;}catch{return false;}}));native.stdin.write(JSON.stringify(ready)+'\n');
  const info=JSON.parse(await line(native,s=>{try{return JSON.parse(s).kind==='ready';}catch{return false;}}));
  workspaceHost=await startWorkspaceHost({endpoint:info.endpoint,transportToken:info.token,picker:true,metadataFile:join(fixture,'workspaces.json'),roots:[{id:'inspector-workspace',cwd:info.workspace,displayName:'Inspector Workspace'}]});
  preview=child(['node',join(checkout,'web-console/node_modules/vite/bin/vite.js'),'preview','--host','127.0.0.1','--port','0','--strictPort'],join(checkout,'web-console'),'preview-node');
  const previewLine=await line(preview,s=>/http:\/\/127\.0\.0\.1:[0-9]+/.test(s));const url=/http:\/\/127\.0\.0\.1:[0-9]+/.exec(previewLine)[0];
  const image='mcr.microsoft.com/playwright:v1.63.0-noble@sha256:bc6ab0d6d44ff4826e4cb8c1e6d801e185bfc42bb0753f8e2a30efc70db054c7';
  container=`pr485-inspector-${process.pid}-${Date.now()}`;
  execFileSync('docker',['run','--detach','--name',container,'--init','--network','host','--ipc','host','--platform','linux/amd64','--security-opt','label=disable','--mount',`type=bind,source=${checkout}/web-console/node_modules,target=/opt/rustx-node_modules,readonly`,image,'sh','-ec','test "$(fc-match --format="%{family}" Helvetica)" = "Liberation Sans"; exec node /opt/rustx-node_modules/@playwright/test/cli.js run-server --host 127.0.0.1 --port 0']);
  let endpoint;
  for(let n=0;n<100&&!endpoint;n++){const log=execFileSync('docker',['logs',container],{encoding:'utf8'});endpoint=/Listening on (ws:\/\/\S+)/.exec(log)?.[1];if(!endpoint)await new Promise(resolve=>setTimeout(resolve,100));}
  assert(endpoint,'pinned browser server ready');await diagnostics.container(container);browser=await chromium.connect(endpoint);browserCdp=await browser.newBrowserCDPSession();
  context=await browser.newContext({viewport:{width:1440,height:1000},locale:'en-US',reducedMotion:'reduce'});page=await context.newPage();
  await page.addInitScript(installBrowserDiagnostics);
  page.on('console',m=>diagnostics.event('browser-console',{type:m.type(),text:m.text().slice(0,512)}));
  page.on('crash',()=>diagnostics.event('browser-crash'));
  page.on('close',()=>diagnostics.event('page-close'));
  await page.addInitScript(()=>localStorage.setItem('rustx-locale-v1','en'));
  page.on('pageerror',e=>errors.push(String(e)));
  await routeWorkspaceHost(page,{workspaceHostUrl:workspaceHost.url});
  await page.routeWebSocket(info.endpoint,route=>{
    const transport=drain.open();
    const upstream=route.connectToServer();browserUpstream=upstream;
    route.onMessage(raw=>{diagnostics.frame('proxy-from-browser',raw,transport);try{observe(JSON.parse(String(raw)),'out',transport);}catch(e){failures.push(String(e));}diagnostics.frame('proxy-to-native',raw,transport);upstream.send(raw);});
    upstream.onMessage(raw=>{
      diagnostics.frame('proxy-from-native',raw,transport);
      const message=JSON.parse(String(raw));try{observe(message,'in',transport);}catch(e){failures.push(String(e));}
      const fixtureReply=fixturePending.get(message.id);
      if(fixtureReply){fixturePending.delete(message.id);message.error?fixtureReply.reject(Error(JSON.stringify(message.error))):fixtureReply.resolve(message.result);return;}
      const request=requests.get(message.id);
      if(holdDetail&&request?.method==='session/traceDetail'&&message.result){holdDetail=false;held={request,message,release:()=>{diagnostics.frame('proxy-held-to-browser',raw,transport);route.send(raw);}};return;}
      diagnostics.frame('proxy-to-browser',raw,transport);route.send(raw);
    });
  });
  await page.goto(url);await connectRemote(page,info.endpoint,info.token);
  await page.locator(`button[data-session-id="${info.session_id}"]`).click();
  await expect(page.locator('#session-view')).toHaveAttribute('data-shell-route','session');
  await expect(page.locator(`button[data-session-id="${info.session_id}"]`)).toHaveAttribute('aria-current','page');
  await expect(page.getByLabel('Message',{exact:true})).toBeEnabled();
  await expect(page.getByLabel('Message',{exact:true})).toHaveJSProperty('readOnly',false);
  await page.getByRole('tab',{name:'Trajectory',exact:true}).click();
  cdp=await context.newCDPSession(page);await rpc.open(info.endpoint,info.token,info.protocol_version);
  await memoryPoint('inspector_open');phase='warm';
  const history=caseName==='short-small'?8:caseName==='short-large'?64:0,turns=caseName==='details-resync'?1:history+32;
  for(let turn=0;turn<turns;turn++){
    if(turn===history){phase='measured';await memoryPoint('before_measured_turns');}
    const before=await counters(native.pid),browserBefore=await browserCpu(),turnStart=performance.now();firstContent=undefined;
    await page.getByLabel('Message',{exact:true}).fill(`benchmark turn ${String(turn).padStart(4,'0')}`);
    await expect(page.getByLabel('Message',{exact:true})).toHaveValue(`benchmark turn ${String(turn).padStart(4,'0')}`);
    await expect(page.getByRole('button',{name:'Send',exact:true})).toBeEnabled();
    await page.getByRole('button',{name:'Send',exact:true}).click();
    if(caseName==='details-resync'){
      await control('observations/await?kind=gate_reached&name=inspector-0&timeoutMs=30000');
      heartbeat=(async()=>{for(let index=0;index<512;index++){
        await control(`observations/await?kind=gate_reached&name=inspector-${index}&timeoutMs=30000`);
        if(!finishHeartbeat)await new Promise(resolve=>setTimeout(resolve,200));
        await control(`gates/inspector-${index}/release`,'POST');
      }})();
      heartbeat.catch(error=>failures.push(`liveness pump: ${error}`));
      await expect.poll(()=>[...traceRecords.values()].filter(r=>r.kind==='request').length).toBe(2);
      const first=[...traceRecords.values()].find(r=>r.kind==='request'&&r.state==='completed');assert(first);
      holdDetail=true;await page.locator(`[data-request-owner="${first.id}"]`).click();
      await expect.poll(()=>!!held).toBe(true);const originalPage=held.message.result.page;
      const browserTarget=[...requests.values()].filter(r=>r.method==='session/subscribe').at(-1).params.target;
      // Append-only P changes the represented Source while the immutable old
      // Request detail response is held; no pending edit/removal occurs yet.
      const add=await rpc.call('session/attachmentStatus',{session_id:info.session_id});assert.equal(add.target,null);
      // Keep the browser's controller as the sole writer for the append probe.
      await page.getByLabel('Message',{exact:true}).fill('inspector queued append proof');
      await page.locator('[data-composer-primary="message"]').click();
      await expect.poll(()=>{
        const request=records.find(x=>x.direction==='out'&&x.method==='turn/start'&&x.params?.content?.some(b=>b.text==='inspector queued append proof'));
        return !!request&&records.some(x=>x.direction==='in'&&x.id===request.id&&x.result);
      }).toBe(true);
      const reread=await browserRead('conversation/checkpointPage',{target:browserTarget,agent_id:null,cut:originalPage.read_cut,domain:originalPage.domain,offset:0,limit:32});
      assert.deepEqual(reread.page,originalPage,'original-cut complete detail page retained after append');
      held.release();held=undefined;
      await expect(page.getByRole('complementary',{name:'Event details',exact:true})).toBeVisible();
      await memoryPoint('held_detail_released');
      phase='resync_pressure';await connectionAction(page,'Disconnect');
      const attached=await rpc.call('session/attach',{session_id:info.session_id,node_id:null});const target=attached.target;
      let checkpoint=(await rpc.call('session/checkpoint',{target,trace_records:[]})).checkpoint;
      let probe=checkpoint.snapshot.inbound.pending.find(item=>item.message.content.some(block=>block.text==='inspector queued append proof'));assert(probe);
      await rpc.call('inbound/remove',{target,expected:{sequence:probe.sequence,revision:probe.revision}});
      const initial=BigInt(checkpoint.cursor);
      for(let index=0;index<2050;index++){
        const accepted=await rpc.call('turn/start',{target,content:[{type:'text',text:`inspector replay pressure ${index}`}]});
        checkpoint=(await rpc.call('session/checkpoint',{target,trace_records:[]})).checkpoint;
        const pending=checkpoint.snapshot.inbound.pending.find(item=>item.sequence===accepted.inbound_sequence);assert(pending,'exact queued occurrence');
        const removed=await rpc.call('inbound/remove',{target,expected:{sequence:pending.sequence,revision:pending.revision}});assert.equal(removed.type,'inbound_mutation');assert.equal(removed.outcome.status,'applied');
      }
      checkpoint=(await rpc.call('session/checkpoint',{target,trace_records:[]})).checkpoint;
      assert(BigInt(checkpoint.cursor)-initial>=4097n,'actual native publication replay pressure');
      assert.equal(checkpoint.snapshot.inbound.pending.length,0);
      await rpc.call('session/detach',{target});await connectionAction(page,'Reconnect');
      await expect.poll(()=>records.some(x=>x.error?.data?.kind==='resync_required'||x.notification?.method==='session/resyncRequired')).toBe(true);
      await expect(page.locator('[data-request-owner][data-state="running"]')).toBeVisible();
      await memoryPoint('after_actual_resync');phase='stream_completion';
      finishHeartbeat=true;await heartbeat;
    }
    await expect.poll(()=>completedAttempts.size).toBe(turn+1);
    await expect.poll(()=>[...traceRecords.values()].filter(r=>r.kind==='request').length).toBe(caseName==='details-resync'?2:turn+1);
    const latest=[...traceRecords.values()].filter(r=>r.kind==='request').at(-1);assert(latest);
    // Keep inspecting the actual current row once the ledger virtualizes,
    // while the independent original detail remains open.
    await page.locator('[data-trajectory-scroll]').evaluate(el=>{el.scrollTop=el.scrollHeight;el.dispatchEvent(new Event('scroll'));});
    await expect(page.locator(`[data-request-owner="${latest.id}"]`)).toHaveAttribute('data-state','completed');
    const after=await counters(native.pid);
    samples.push({turn,measured:turn>=history,accepted_and_settled_ms:performance.now()-turnStart,first_content_ms:firstContent==null?null:firstContent-turnStart,before,after,browserBefore,browserAfter:await browserCpu()});
    // Preserve the existing current-row timing/counters above. The added
    // endpoint includes all loaded repair work, not just the visible row.
    const ui=samples.at(-1);let proof;
    await expect.poll(async()=>{
      const before=drain.proof(traceRecords);
      if(!before.ready)return false;
      // Ack delivery through the actual browser event loop and two paints.
      // No extra Native RPC, publication, timer, or product test hook.
      await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
      const after=drain.proof(traceRecords);
      if(!after.ready||after.generation!==before.generation)return false;
      proof=after;return true;
    },{message:'all loaded Trace rows repaired at the latest observed cut, no pending browser RPC, browser acknowledged'}).toBe(true);
    assert([...traceRecords.values()].filter(r=>r.kind==='request').every(r=>r.state==='completed'));
    ui.full_repair_end_to_end_ms=performance.now()-turnStart;
    ui.repair_tail_after_current_row_ms=ui.full_repair_end_to_end_ms-ui.accepted_and_settled_ms;
    ui.full_repair_after=await counters(native.pid);
    ui.full_repair_browser_after=await browserCpu();
    ui.full_repair_proof=proof;
    if(turn===0&&caseName!=='details-resync'){
      await page.locator(`[data-request-owner="${latest.id}"]`).click();
      await expect(page.getByRole('complementary',{name:'Event details',exact:true})).toBeVisible();
      await memoryPoint('original_detail_held');
    }
  }
  await memoryPoint('after_measured_turns');
  const observed=(await control('requests')).requests;const expectedCount=caseName==='details-resync'?2:turns;assert.equal(observed.length,expectedCount);
  for(let index=0;index<observed.length;index++)assert.deepEqual(normalized(observed[index].body,info.workspace,true),expected(hostCase,index,normativeTools));
  native.stdin.write(JSON.stringify({op:'verify',requests:observed})+'\n');await line(native,s=>{try{return JSON.parse(s).kind==='verified';}catch{return false;}});
  phase='retirement';await diagnostics.captureBrowser(page);await page.close();page=undefined;await context.close();context=undefined;await browser.close();browser=undefined;await rpc.close();await workspaceHost.stop();workspaceHost=undefined;
  native.stdin.write('{"op":"stop"}\n');native.stdin.end();complete=JSON.parse(await line(native,s=>{try{return JSON.parse(s).kind==='complete';}catch{return false;}}));assert.equal((await native.closed).code,0);
  const canonicalText=complete.canonical.filter(m=>m.role==='assistant').flatMap(m=>m.content.filter(b=>b.type==='text').map(b=>b.text));
  assert.deepEqual(canonicalText,caseName==='details-resync'?[stream]:Array.from({length:turns},(_,i)=>reply(i)),'complete canonical output differs from the independent fixture');
  const tools=complete.canonical.filter(m=>m.role==='tool');
  assert.equal(tools.length,caseName==='details-resync'?1:0);
  for(const tool of tools){assert.equal(tool.result.status.type,'success');assert.deepEqual(tool.result.content,[{type:'text',text:'deterministic-note-payload-6d41'}]);}
  provider.stdin.end();const reportLine=await line(provider,s=>{try{return !!JSON.parse(s).report;}catch{return false;}});report=JSON.parse(reportLine).report;assert.equal((await provider.closed).code,0);assert(report.ok);assert.deepEqual(report.requests,observed);
  assert.deepEqual(errors,[]);assert.deepEqual(failures,[]);
  await writeFile(join(output,'sample.json'),JSON.stringify({inspector_workload_version:3,drain_observations:drain.events,repair_drain_criterion:"all-loaded-latest-cut/no-pending-RPC/two-browser-paints",ui_metric:"current-row completion; does not alone establish repair drain",fixture_source,performance_eligible:false,case:caseName,oracle:'passed',source:JSON.parse(await readFile(join(checkout,'source-identity.json'),'utf8')).head,private_proposal:JSON.parse(await readFile(join(checkout,'source-identity.json'),'utf8')).proposal??null,native_binary_sha256:hash(await readFile(binary)),model_time:MODEL_TIME,samples,memory,pressure,complete,provider_report:report,records,errors,failures},null,2));
}catch(error){diagnostics.event('fixture-failure',{error:String(error)});await diagnostics.captureBrowser(page);failures.push(String(error));const body=page?await page.locator('body').innerText().catch(()=>null):null;await writeFile(join(output,'failure.json'),JSON.stringify({error:String(error),stack:error.stack,body,phase,samples,memory,records,errors,failures,native_errors:native?.errors,provider_errors:provider?.errors},null,2));process.exitCode=1;}
finally{
  if(page)await page.close().catch(()=>{});if(context)await context.close().catch(()=>{});if(browser)await browser.close().catch(()=>{});await rpc.close().catch(()=>{});if(workspaceHost)await workspaceHost.stop().catch(()=>{});
  if(native&&native.exitCode===null)native.kill('SIGTERM');if(provider&&provider.exitCode===null)provider.stdin.end();if(preview&&preview.exitCode===null)preview.kill('SIGTERM');if(container)try{execFileSync('docker',['rm','-f',container],{stdio:'ignore'});}catch{}
  for(const [name,p] of [['native',native],['provider',provider],['preview',preview]])if(p)await writeFile(join(output,name+'.stderr'),p.errors);
  await diagnostics.finish(join(output,'diagnostics.json'));
}
