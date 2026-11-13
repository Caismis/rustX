/** Measurements only. Uses the existing typed product protocol client, never a new SDK/demo. */
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {spawn,execFileSync} from 'node:child_process';
import {mkdtempSync,mkdirSync,writeFileSync,readFileSync,rmSync,existsSync} from 'node:fs';
import {tmpdir,arch,cpus} from 'node:os';
import {join,resolve} from 'node:path';
import {once} from 'node:events';
import {createInterface} from 'node:readline';
import {AppServerHost} from '../../tui/src/app-server/host.ts';
import {ProviderEmulator} from '../../tui/test/support/provider-emulator.ts';
import {counters,processTree,slowProxy} from './process.mts';
const binary=resolve(process.env.RUSTX_BINARY??'target/debug/rustx'),cycles=20;
assert.equal(process.platform,'linux');assert.ok(existsSync(binary));
const ticksPerSecond=Number(execFileSync('getconf',['CLK_TCK'],{encoding:'utf8'}).trim());
const measurements=[];
for(const mode of [{name:'environment-one-client',clients:1,sessions:1,slow:false},{name:'environment-three-clients',clients:3,sessions:1,slow:false},{name:'environment-three-sessions',clients:3,sessions:3,slow:false},{name:'environment-one-idle-paused-client',clients:3,sessions:1,slow:true}]) {
  // Reuse the existing empty script: every unexpected model request is a failure.
  const provider=await ProviderEmulator.start('web_config_repair');
  const root=mkdtempSync(join(tmpdir(),'rustx-environment-measure-')),home=join(root,'home'),workspace=join(root,'workspace');mkdirSync(home);mkdirSync(workspace);
  const token='environment-measurement-private-token';writeFileSync(join(root,'token'),token,{mode:0o600});
  writeFileSync(join(root,'rustx.toml'),`[app_server]\nenvironment_roots=[${JSON.stringify(workspace)}]\n[agent.model]\nmodel="measure/chat-model"\n[agent.tools]\nbuiltin=[]\n[providers.measure]\nbase_url=${JSON.stringify(provider.url())}\napi_key="$MEASURE_KEY"\n[models."measure/chat-model"]\nprovider="measure"\nid="chat-model"\nprotocol="openai_chat_completions"\ncontext_window=128000\nmax_output_tokens=1024\n[models."measure/chat-model".capabilities]\ninput_modalities=["text"]\noutput_modalities=["text"]\n[models."measure/chat-model".compat]\nchat_reasoning_replay="omit"\n`);
  const child=spawn(binary,['app-server','--config',join(root,'rustx.toml'),'--runtime-root',join(root,'runtime'),'--listen','ws://127.0.0.1:0','--token-file',join(root,'token')],{env:{...process.env,HOME:home,MEASURE_KEY:'fixture'},stdio:['ignore','ignore','pipe']});
  const exited=once(child,'exit'),hosts:AppServerHost[]=[],workspaces:string[]=[];
  let proxy:Awaited<ReturnType<typeof slowProxy>>|undefined;
  const bounded=<T>(promise:Promise<T>)=>new Promise<T>((resolve,reject)=>{const timer=setTimeout(()=>reject(Error('Measurement operation timed out')),120_000);promise.then(value=>{clearTimeout(timer);resolve(value);},error=>{clearTimeout(timer);reject(error);});});
  try {
    const endpoint=await bounded(new Promise<string>((resolve,reject)=>{const lines=createInterface({input:child.stderr});let diagnostic='';lines.on('line',line=>{diagnostic=(diagnostic+'\n'+line).slice(-4096);if(line.startsWith('rustx app-server listening '))resolve(line.slice('rustx app-server listening '.length));});child.once('exit',()=>reject(Error(diagnostic)));}));
    if(mode.slow)proxy=await slowProxy(endpoint);
    for(let client=0;client<mode.clients;client++)hosts.push(await AppServerHost.connectRemote({endpoint:mode.slow&&client===mode.clients-1?proxy!.endpoint:endpoint,token}));
    for(let index=0;index<mode.sessions;index++) {const cwd=join(workspace,`session-${index}`);mkdirSync(cwd);workspaces.push(cwd);await hosts[0].client.call('thread/start',{settings:{cwd},dynamicTools:[]},'session_transition');}
    const activeClients=mode.slow?hosts.length-1:hosts.length;
    const cycle=async(index:number)=>{
      const client=hosts[Math.abs(index)%activeClients].client,cwd=workspaces[Math.abs(index)%workspaces.length],path=join(cwd,`cycle-${index}`),watchId=`cycle-${index}`;
      const {metadata}=await client.call('fs/writeFile',{path,dataBase64:Buffer.from('measured').toString('base64'),expectedRevision:null},'file_metadata');
      const {chunk}=await client.call('fs/readFile',{path,offset:'0',maxBytes:64,expectedRevision:metadata.revision},'file_chunk');assert.equal(Buffer.from(chunk.dataBase64,'base64').toString(),'measured');
      await client.call('fs/watch',{watchId,path:cwd},'environment_accepted');
      const {result}=await client.call('command/exec',{command:['/usr/bin/printf','%s','measured'],cwd,env:{EXPLICIT:'authorized'},processId:null,tty:false,streamStdin:false,streamStdoutStderr:false,outputBytesCap:65536,timeoutMs:10000,size:null},'command_exec');
      assert.equal(result.status,'completed');assert.equal(Buffer.from(result.stdoutBase64,'base64').toString(),'measured');
      await client.call('fs/unwatch',{watchId},'environment_accepted');await client.call('fs/remove',{path,recursive:false,force:false},'environment_accepted');
    };
    for(let index=-5;index<0;index++)await bounded(cycle(index));
    proxy?.pause();
    const before=counters(child.pid!),latencies:number[]=[],descriptorSamples:number[]=[],started=performance.now();
    for(let index=0;index<cycles;index++) {const start=performance.now();await bounded(cycle(index));latencies.push(performance.now()-start);descriptorSamples.push(counters(child.pid!).descriptors);assert.equal(processTree(child.pid!).length,1,'No supervised command descendants may survive completion');}
    const after=counters(child.pid!),elapsedMs=performance.now()-started;
    const client=hosts[0].client,ready=new Map<string,{resolve:()=>void;text:string}>();let outputBytes=0;
    const unsubscribe=client.onNotification(notification=>{if(notification.method!=='command/exec/outputDelta')return;const delta=notification.params;const bytes=Buffer.from(delta.deltaBase64,'base64');outputBytes+=bytes.length;const waiter=ready.get(delta.processId);if(waiter){waiter.text+=bytes.toString();if(waiter.text.includes('ready'))waiter.resolve();}});
    const completions:Promise<unknown>[]=[],barriers:Promise<void>[]=[];
    for(let index=0;index<4;index++){const processId=`peak-${index}`;barriers.push(new Promise<void>(resolve=>ready.set(processId,{resolve,text:''})));completions.push(client.call('command/exec',{command:['/bin/sh','-c','printf ready; read line; printf done'],cwd:workspaces[0],env:{},processId,tty:false,streamStdin:true,streamStdoutStderr:true,outputBytesCap:65536,timeoutMs:30000,size:null},'command_exec'));}
    await bounded(Promise.all(barriers));
    for(let index=0;index<8;index++)await client.call('fs/watch',{watchId:`peak-${index}`,path:workspaces[0]},'environment_accepted');
    const peak=counters(child.pid!),tree=processTree(child.pid!);let treeResidentKiB=0;for(const pid of tree)try{treeResidentKiB+=counters(pid).residentKiB;}catch{}
    for(let index=0;index<4;index++)await client.call('command/exec/write',{processId:`peak-${index}`,deltaBase64:Buffer.from('finish\n').toString('base64'),closeStdin:true},'environment_accepted');
    await bounded(Promise.all(completions));unsubscribe();
    for(let index=0;index<8;index++)await client.call('fs/unwatch',{watchId:`peak-${index}`},'environment_accepted');
    await client.call('server/info',{},'server_info');
    const cleaned=counters(child.pid!);assert.equal(processTree(child.pid!).length,1);
    const providerRequests=(await provider.requests()).length;assert.equal(providerRequests,0,'Environment operations never invoke a model');
    const ordered=[...latencies].sort((a,b)=>a-b),percentile=(q:number)=>ordered[Math.ceil(q*ordered.length)-1];
    const result={...mode,cycles,hostTools:0,providerRequests,elapsedMs,cpuMs:(after.cpuTicks-before.cpuTicks)*1000/ticksPerSecond,reapedChildCpuMs:(after.childCpuTicks-before.childCpuTicks)*1000/ticksPerSecond,residentKiBBefore:before.residentKiB,residentKiBAfter:after.residentKiB,peakResidentKiB:peak.peakResidentKiB,kernelWriteCalls:after.writeCalls-before.writeCalls,kernelWrittenBytes:after.writtenBytes-before.writtenBytes,kernelWrittenCharacters:after.writtenCharacters-before.writtenCharacters,latencyMs:{p50:percentile(.5),p95:percentile(.95),samples:latencies},resources:{descriptorsBefore:before.descriptors,descriptorsAfterCycles:after.descriptors,descriptorSamples,admittedCommandsAtPeak:4,admittedWatchesAtPeak:8,descriptorsAtPeak:peak.descriptors,processTreeSizeAtPeak:tree.length,summedProcessTreeResidentKiB:treeResidentKiB,streamedBytes:outputBytes,descriptorsAfterCleanup:cleaned.descriptors,processTreeSizeAfterCleanup:processTree(child.pid!).length}};
    measurements.push(result);console.log(JSON.stringify(result));
  } finally {
    proxy?.resume();await Promise.allSettled(hosts.map(host=>host.shutdown()));await proxy?.close();child.kill('SIGTERM');await exited;rmSync(root,{recursive:true,force:true});await provider.finish();
  }
}
const paths=execFileSync('git',['ls-files','--cached','--others','--exclude-standard','-z','src','Cargo.toml','Cargo.lock','protocol/app-server','tui/src/app-server','benchmarks/app-server/environment.mts','benchmarks/app-server/process.mts'],{encoding:'utf8'}).split('\0').filter(Boolean).filter(path=>existsSync(path)).sort();
const hash=createHash('sha256');for(const path of [...new Set(paths)])hash.update(path).update('\0').update(readFileSync(path)).update('\0');
const output=resolve(process.argv[2]??'/tmp/rustx-environment-measurements.json');writeFileSync(output,JSON.stringify({revision:execFileSync('git',['rev-parse','HEAD'],{encoding:'utf8'}).trim(),workingTree:true,measuredSourceSha256:hash.digest('hex'),binarySha256:createHash('sha256').update(readFileSync(binary)).digest('hex'),binaryProfile:'debug --all-features',kernel:execFileSync('uname',['-srvm'],{encoding:'utf8'}).trim(),arch:arch(),cpu:cpus()[0]?.model,ticksPerSecond,methodology:'Five warmups, then 20 sequential atomic write/read/watch/argv/unwatch/remove cycles. Native metadata-only Sessions; no Host Tools and zero model calls. CPU/write deltas exclude setup and the separate four-command/eight-watch peak. A paused idle peer is not a saturated output proof. Process tree RSS is a sum with shared pages counted repeatedly. Non-isolated developer host; no timing assertions or capacity guarantee.',measurements},null,2)+'\n');
