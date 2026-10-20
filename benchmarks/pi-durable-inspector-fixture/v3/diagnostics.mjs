// Diagnostic observations only. No workload, application or transport decisions.
import {readFile, writeFile} from 'node:fs/promises';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {monitorEventLoopDelay, performance} from 'node:perf_hooks';
const run = promisify(execFile);

export function frameMeta(raw) {
  const text = typeof raw === 'string' ? raw : Buffer.isBuffer(raw) ? raw.toString() : '';
  const value = {bytes: Buffer.byteLength(text)};
  try {
    const m = JSON.parse(text);
    if (typeof m.id === 'string' || typeof m.id === 'number') value.id = m.id;
    if (typeof m.method === 'string') value.method = m.method;
    if (typeof m.result?.type === 'string') value.result_type = m.result.type;
    if (m.error) value.error_code = m.error.code;
  } catch { value.json = 'unparsed'; }
  return value; // Never retain request/response bodies, credentials or protocols.
}

export class Diagnostics {
  events = []; dropped = 0; roles = new Map([['fixture-node', process.pid]]);
  browser = {status: 'not-collected'}; busy = false; exits = [];
  constructor() {
    this.started = performance.now();
    this.loop = monitorEventLoopDelay({resolution: 20}); this.loop.enable();
    this.timer = setInterval(() => { if(!this.pendingSample)this.pendingSample=this.poll().finally(()=>{this.pendingSample=undefined;}); }, 1000); this.timer.unref();
  }
  event(kind, data = {}) {
    if (this.events.length >= 50000) { this.dropped++; return; }
    this.events.push({at_ms: performance.now()-this.started, utc_ms: Date.now(), kind, ...data});
  }
  frame(kind, raw, epoch) { this.event(kind, {epoch, ...frameMeta(raw)}); }
  child(role, child) {
    this.roles.set(role, child.pid); this.event('child-start', {role, pid: child.pid});
    this.exits.push(new Promise(resolve=>child.on('exit', (code, signal) => {this.event('child-exit', {role, pid: child.pid, code, signal});resolve();})));
  }
  async container(name) {
    try {
      const {stdout} = await run('docker', ['top', name, '-eo', 'pid,args'], {timeout: 1000});
      const rows = stdout.split('\n').filter(line => /node .*playwright.*cli\.js run-server/.test(line));
      for (const [index, line] of rows.entries()) {
        const pid = Number(line.trim().split(/\s+/)[0]);
        if (Number.isSafeInteger(pid)) this.roles.set('playwright-server-node-'+index, pid);
      }
      this.event('container-node-discovery', {matched: rows.length});
    } catch (error) { this.event('container-node-unavailable', {error: String(error)}); }
  }
  async poll() {
    if (this.busy) return;
    this.busy = true;
    try {
      const records = [];
      for (const [role, pid] of this.roles) {
        const ids = [[role, pid]];
        const children = role==='provider-command'?await readFile(`/proc/${pid}/task/${pid}/children`, 'utf8').catch(()=>''):'';
        for (const child of children.trim().split(/\s+/).filter(Boolean)) ids.push([role+'-child', Number(child)]);
        for (const [name, id] of ids) {
          records.push({role:name, pid:id, stat:await readFile(`/proc/${id}/stat`,'utf8').catch(()=>null), io:await readFile(`/proc/${id}/io`,'utf8').catch(()=>null)});
        }
      }
      this.event('process-counters', {records, fixture_cpu_us:process.cpuUsage(),
        loop_delay_ns:{max:this.loop.max, mean:Number.isFinite(this.loop.mean)?this.loop.mean:null, p99:this.loop.percentile(99)}});
      this.loop.reset();
    } finally { this.busy = false; }
  }
  async captureBrowser(page) {
    if (!page) return;
    let timer;
    try {
      // Only diagnostic collection has this bound; all original action/RPC/workload deadlines remain unchanged.
      this.browser = await Promise.race([page.evaluate(()=>globalThis.__pr485Diagnostic??{status:'missing'}),
        new Promise(resolve=>{timer=setTimeout(()=>resolve({status:'collection-timeout'}),1000);})]);
    } catch (error) { this.browser={status:'unavailable',error:String(error)}; }
    finally { clearTimeout(timer); }
  }
  async finish(path) {
    clearInterval(this.timer); this.loop.disable();
    let timer;
    await Promise.race([Promise.allSettled([...this.exits,this.pendingSample]),new Promise(resolve=>{timer=setTimeout(()=>{this.event('diagnostic-exit-collection-timeout');resolve();},1000);})]);
    clearTimeout(timer);
    this.event('diagnostic-finish');
    await writeFile(path,JSON.stringify({version:1,performance_eligible:false,
      scope:'Diagnostic only; observers add overhead. Proxy timestamps include Playwright bridge scheduling, not exclusive Native service time. PID counters are sampled and may miss exit CPU. WorkspaceHost shares fixture-node. Remote Playwright Node event-loop delay is not directly observed. Missing/dropped observations are explicit.',
      clock_ticks:100,events:this.events,dropped:this.dropped,browser:this.browser},null,2));
  }
}

/** Installed before the application. Preserve every original send/close argument. */
export function installBrowserDiagnostics() {
  const state={status:'collected',time_origin_ms:performance.timeOrigin,events:[],dropped:0};
  globalThis.__pr485Diagnostic=state;
  const event=(kind,data={})=>{
    if(state.events.length>=50000){state.dropped++;return;}
    state.events.push({at_ms:performance.now(),utc_ms:Date.now(),kind,...data});
  };
  const meta=raw=>{
    const text=typeof raw==='string'?raw:'',value={bytes:typeof raw==='string'?new TextEncoder().encode(text).length:null};
    try{const m=JSON.parse(text);if(typeof m.id==='string'||typeof m.id==='number')value.id=m.id;if(typeof m.method==='string')value.method=m.method;if(typeof m.result?.type==='string')value.result_type=m.result.type;}catch{}
    return value;
  };
  const Original=globalThis.WebSocket;let sequence=0;
  globalThis.WebSocket=class extends Original {
    constructor(url,protocols){
      super(...(protocols===undefined?[url]:[url,protocols]));this.diagnosticId=++sequence;
      event('ws-created',{socket:this.diagnosticId});
      this.addEventListener('open',()=>event('ws-open',{socket:this.diagnosticId}));
      this.addEventListener('message',e=>event('ws-receive-dispatched',{socket:this.diagnosticId,...meta(e.data)}));
      this.addEventListener('error',()=>event('ws-error',{socket:this.diagnosticId}));
      this.addEventListener('close',e=>event('ws-close',{socket:this.diagnosticId,code:e.code,reason:e.reason,wasClean:e.wasClean}));
    }
    send(data){event('ws-send',{socket:this.diagnosticId,...meta(data)});return super.send(data);}
    close(...args){event('ws-close-requested',{socket:this.diagnosticId,code:args[0]??null});return super.close(...args);}
  };
  let previous;
  const changed=()=>{
    const input=document.querySelector('textarea[aria-label="Message"]');
    const value={present:!!input,disabled:input?.disabled??null,readOnly:input?.readOnly??null};
    const key=JSON.stringify(value);if(key!==previous){previous=key;event('composer-state',value);}
  };
  const observe=()=>{changed();new MutationObserver(changed).observe(document.documentElement,{subtree:true,childList:true,attributes:true,attributeFilter:['disabled','readonly']});};
  if(document.documentElement)observe();else document.addEventListener('DOMContentLoaded',observe,{once:true});
}
