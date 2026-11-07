/** Linux process measurements of the real App Server and existing TUI protocol client.
 * Run from the repository root after building the actual binary:
 * RUSTX_REQUIRE_PROVIDER_EMULATOR=1 node benchmarks/app-server/measure.mts /tmp/app-server-measurements.json
 * No timing assertions. Kernel process I/O counters are not physical-device I/O.
 */
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {spawn,execFileSync} from 'node:child_process';
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync,existsSync} from 'node:fs';
import {tmpdir,arch,cpus} from 'node:os';
import {join,resolve} from 'node:path';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {createServer,connect,type Socket} from 'node:net';
import {AppServerHost} from '../../tui/src/app-server/host.ts';
import type {AppServerSession} from '../../tui/src/app-server/session.ts';
import {ProviderEmulator} from '../../tui/test/support/provider-emulator.ts';
const binary=resolve(process.env.RUSTX_BINARY??'target/debug/rustx'),turns=20;
assert.equal(process.platform,'linux','This measurement reads Linux /proc process counters.');
assert.ok(existsSync(binary),'Build the actual binary first.');
const ticksPerSecond=Number(execFileSync('getconf',['CLK_TCK'],{encoding:'utf8'}).trim());
function counters(pid:number) {
  const stat=readFileSync(`/proc/${pid}/stat`,'utf8');
  const fields=stat.slice(stat.lastIndexOf(')')+2).trim().split(/\s+/);
  const io=Object.fromEntries(readFileSync(`/proc/${pid}/io`,'utf8').trim().split('\n').map(line=>{const[k,v]=line.split(':');return[k,Number(v.trim())];}));
  const status=readFileSync(`/proc/${pid}/status`,'utf8');
  return {cpuTicks:Number(fields[11])+Number(fields[12]),writeCalls:io.syscw,writtenBytes:io.write_bytes,writtenCharacters:io.wchar,
    residentKiB:Number(status.match(/^VmRSS:\s+(\d+)/m)![1]),peakResidentKiB:Number(status.match(/^VmHWM:\s+(\d+)/m)![1])};
}
async function slowProxy(endpoint:string) {
  const url=new URL(endpoint),links:Array<{client:Socket;server:Socket}>=[];
  const listener=createServer(client=>{
    const server=connect(Number(url.port),url.hostname);client.pipe(server);server.pipe(client);
    client.on('error',()=>server.destroy());server.on('error',()=>client.destroy());
    client.on('close',()=>server.destroy());server.on('close',()=>client.destroy());links.push({client,server});
  });
  listener.listen(0,'127.0.0.1');await once(listener,'listening');
  const address=listener.address();assert.ok(address&&typeof address==='object');
  return {endpoint:`ws://127.0.0.1:${address.port}/`,pause:()=>{for(const link of links){link.server.unpipe(link.client);link.server.pause();}},
    resume:()=>{for(const link of links){link.server.pipe(link.client);link.server.resume();}},close:async()=>{for(const link of links){link.client.destroy();link.server.destroy();}await new Promise<void>(resolve=>listener.close(()=>resolve()));}};
}
function settled(session:AppServerSession,previousTurnId:string|undefined):Promise<void> {
  const done=()=>session.state.attempt!==undefined&&session.state.attempt.attemptId!==previousTurnId&&session.state.attempt.phase.type==='settled';
  if(done())return Promise.resolve();
  return new Promise<void>((resolve,reject)=>{
    const timer=setTimeout(()=>{stop();reject(Error('Native Attempt did not settle.'));},120_000);
    const stop=session.onState(()=>{if(done()){clearTimeout(timer);stop();resolve();}});
  });
}
const modes=[
  {name:'no-host-tools',host:false,clients:1,sessions:1,slow:false},
  {name:'host-one-client',host:true,clients:1,sessions:1,slow:false},
  {name:'host-three-clients',host:true,clients:3,sessions:1,slow:false},
  {name:'host-three-clients-one-paused',host:true,clients:3,sessions:1,slow:true},
  {name:'host-three-resident-sessions',host:true,clients:3,sessions:3,slow:false},
];
const measurements=[];
for(const mode of modes) {
  const provider=await ProviderEmulator.start(mode.host?'app_server_measure_host':'app_server_measure_no_host');
  const root=mkdtempSync(join(tmpdir(),'rustx-app-server-measure-'));mkdirSync(join(root,'home'));
  const token='measurement-private-token';writeFileSync(join(root,'token'),token,{mode:0o600});
  writeFileSync(join(root,'rustx.toml'),`
[agent.model]
model="measure/chat-model"
[agent.tools]
builtin=[]
[providers.measure]
base_url=${JSON.stringify(provider.url())}
api_key="$MEASURE_KEY"
[models."measure/chat-model"]
provider="measure"
id="chat-model"
protocol="openai_chat_completions"
context_window=128000
max_output_tokens=1024
[models."measure/chat-model".capabilities]
input_modalities=["text"]
output_modalities=["text"]
[models."measure/chat-model".compat]
chat_reasoning_replay="omit"
`);
  const child=spawn(binary,['app-server','--config',join(root,'rustx.toml'),'--runtime-root',join(root,'runtime'),'--listen','ws://127.0.0.1:0','--token-file',join(root,'token')],
    {env:{...globalThis.process.env,HOME:join(root,'home'),XDG_CONFIG_HOME:undefined,XDG_STATE_HOME:undefined,MEASURE_KEY:'fixture'},stdio:['ignore','ignore','pipe']});
  const exited=once(child,'exit'),hosts:AppServerHost[]=[],sessions:AppServerSession[]=[];
  let originalFailure:unknown;
  let proxy:Awaited<ReturnType<typeof slowProxy>>|undefined;
  try {
    const endpoint=await new Promise<string>((resolve,reject)=>{
      const lines=createInterface({input:child.stderr});let diagnostic='';
      lines.on('line',line=>{diagnostic=(diagnostic+'\n'+line).slice(-4096);if(line.startsWith('rustx app-server listening '))resolve(line.slice('rustx app-server listening '.length));});
      child.once('exit',()=>reject(Error(diagnostic)));
    });
    if(mode.slow)proxy=await slowProxy(endpoint);
    for(let client=0;client<mode.clients;client++) {
      const host=await AppServerHost.connectRemote({endpoint:mode.slow&&client===mode.clients-1?proxy!.endpoint:endpoint,token});
      hosts.push(host);
      if(mode.host&&!(mode.slow&&client===mode.clients-1))host.client.callbacks.installHandler('business','lookup',async()=>({success:true,contentItems:[{type:'inputText',text:'measured'}]}));
    }
    for(let index=0;index<mode.sessions;index++) {
      const cwd=join(root,`workspace-${index}`);mkdirSync(cwd);
      const created=await hosts[0].client.call('thread/start',{settings:{cwd},dynamicTools:mode.host?[{namespace:'business',name:'lookup',description:'Measure application callback',inputSchema:{type:'object',properties:{key:{type:'string'}},required:['key'],additionalProperties:false}}]:[]},'session_transition');
      await hosts[0].client.call('session/name',{session_id:created.session.id,name:`Measurement ${index}`},'session');
      for(let client=0;client<hosts.length;client++) {
        const session=await hosts[client].attach(created.session.id,created.thread.nodeId,undefined,created.thread.id);
        if(client===0)sessions.push(session);
      }
    }
    proxy?.pause();
    const before=counters(child.pid!),latencies:number[]=[],started=performance.now();
    for(let index=0;index<turns;index++) {
      const session=sessions[index%sessions.length],at=performance.now();
      const previousTurnId=session.state.attempt?.attemptId;
      await hosts[0].client.call('inbound/queue',{target:session.target,inputId:`measure-${index}`,content:[{type:'text',text:`measure-input ${index}`}]},'inbound_accepted');
      await settled(session,previousTurnId);latencies.push(performance.now()-at);
    }
    const elapsedMs=performance.now()-started,after=counters(child.pid!),requests=(await provider.requests()).length;
    assert.equal(requests,turns*(mode.host?2:1));
    let nativeTurns=0;
    for(const session of sessions) {
      const {page}=await hosts[0].client.call('session/turns',{target:session.target,offset:0,limit:64},'conversation_turns');
      nativeTurns+=Number(page.total);
    }
    assert.equal(nativeTurns,turns,'Tool continuations must not create additional public Turns.');
    const ordered=[...latencies].sort((a,b)=>a-b),percentile=(q:number)=>ordered[Math.ceil(q*ordered.length)-1];
    const result={...mode,turns,nativeTurns,providerRequests:requests,elapsedMs,cpuMs:(after.cpuTicks-before.cpuTicks)*1000/ticksPerSecond,
      residentKiBBefore:before.residentKiB,residentKiBAfter:after.residentKiB,peakResidentKiB:after.peakResidentKiB,
      kernelWriteCalls:after.writeCalls-before.writeCalls,kernelWrittenBytes:after.writtenBytes-before.writtenBytes,kernelWrittenCharacters:after.writtenCharacters-before.writtenCharacters,
      latencyMs:{p50:percentile(.5),p95:percentile(.95),samples:latencies}};
    measurements.push(result);console.log(JSON.stringify(result));
  } catch(error) {originalFailure=error;throw error;} finally {
    try {
      proxy?.resume();
      await Promise.allSettled(hosts.map(host=>host.shutdown()));
      await proxy?.close();
    } finally {
      child.kill('SIGTERM');
      try { await exited; } finally { rmSync(root,{recursive:true,force:true}); try {await provider.finish();} catch(error) {if(originalFailure)console.error('Provider cleanup incomplete after original failure.');else throw error;} }
    }
  }
}
const sourcePaths=execFileSync('git',['ls-files','-z','src','Cargo.toml','Cargo.lock','protocol/app-server','tui/src/app-server','test-support/fake-provider/src/fake_provider/scenarios/app_server.py','benchmarks/app-server/measure.mts'],{encoding:'utf8'}).split('\0').filter(Boolean).filter(path=>existsSync(path));
const codeHash=createHash('sha256');for(const path of sourcePaths)codeHash.update(path).update('\0').update(readFileSync(path)).update('\0');
const output=resolve(globalThis.process.argv[2]??'/tmp/rustx-app-server-measurements.json');
writeFileSync(output,JSON.stringify({revision:execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim(),workingTree:true,measuredSourceSha256:codeHash.digest('hex'),binarySha256:createHash('sha256').update(readFileSync(binary)).digest('hex'),binaryProfile:'debug --all-features',
  kernel:execFileSync('uname',['-srvm'],{encoding:'utf8'}).trim(),arch:arch(),cpu:cpus()[0]?.model,ticksPerSecond,
  methodology:'Fresh process per mode; setup and shutdown excluded from deltas; 20 sequential queued inputs, each observed in its own settled native Attempt. Multiple Sessions are round-robin with all clients subscribed. The paused client is behind a paused transparent TCP proxy. /proc process write counters are kernel accounting, not physical-device I/O. No latency assertions.',measurements},null,2)+'\n');
