import test from 'node:test';
import assert from 'node:assert/strict';
import {EventEmitter} from 'node:events';
import {mkdtemp,readFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {Diagnostics,frameMeta,installBrowserDiagnostics} from './diagnostics.mjs';

test('frame metadata excludes bodies and credentials',()=>{
  const raw=JSON.stringify({id:'0:45',method:'session/trace',params:{token:'do-not-log',content:'private-body'}});
  const value=frameMeta(raw);
  assert.deepEqual(value,{bytes:Buffer.byteLength(raw),id:'0:45',method:'session/trace'});
  assert(!JSON.stringify(value).includes('do-not-log'));
});

test('browser observer forwards original arguments and records lifecycle/state',()=>{
  const previous={WebSocket:globalThis.WebSocket,document:globalThis.document,MutationObserver:globalThis.MutationObserver};
  let mutation;const input={disabled:false,readOnly:false};
  class Socket extends EventTarget {
    static OPEN=1;
    constructor(...args){super();this.originalArgs=args;this.sent=[];}
    send(value){this.sent.push(value);return 'sent';}
    close(...args){this.closed=args;return 'closed';}
  }
  try{
    globalThis.WebSocket=Socket;
    globalThis.document={documentElement:{},querySelector:()=>input};
    globalThis.MutationObserver=class{constructor(fn){mutation=fn;}observe(){}};
    installBrowserDiagnostics();
    const ws=new WebSocket('ws://local',['opaque-token-protocol']);
    const raw=JSON.stringify({id:'1',method:'session/trace',params:{token:'do-not-log'}});
    assert.equal(ws.send(raw),'sent');assert.deepEqual(ws.sent,[raw]);
    assert.deepEqual(ws.originalArgs,['ws://local',['opaque-token-protocol']]);
    ws.dispatchEvent(new MessageEvent('message',{data:'{"id":"1","result":{"type":"trace","private":"do-not-log"}}'}));
    input.disabled=true;mutation();assert.equal(ws.close(1000,'normal'),'closed');assert.deepEqual(ws.closed,[1000,'normal']);
    assert.equal(WebSocket.OPEN,1);
    const events=globalThis.__pr485Diagnostic.events;
    assert(events.some(x=>x.kind==='composer-state'&&x.disabled));
    assert(events.some(x=>x.kind==='ws-receive-dispatched'&&x.id==='1'));
    assert(events.some(x=>x.kind==='ws-close-requested'&&x.code===1000));
    assert(!JSON.stringify(events).includes('do-not-log'));assert(!JSON.stringify(events).includes('opaque-token-protocol'));
  }finally{Object.assign(globalThis,previous);delete globalThis.__pr485Diagnostic;}
});

test('bounded capture retains exit status and makes overflow explicit',async()=>{
  const root=await mkdtemp(join(tmpdir(),'pr485-diagnostic-unit-'));const file=join(root,'diagnostics.json');
  try{
    const capture=new Diagnostics();const child=new EventEmitter();child.pid=99999999;capture.child('synthetic-child',child);child.emit('exit',7,'SIGTERM');
    capture.events.length=50000;capture.event('overflow');assert.equal(capture.dropped,1);
    capture.events=capture.events.filter(Boolean);await capture.finish(file);
    const data=JSON.parse(await readFile(file,'utf8'));assert.equal(data.performance_eligible,false);assert.equal(data.dropped,1);
    assert(data.events.some(x=>x.kind==='child-exit'&&x.code===7&&x.signal==='SIGTERM'));
  }finally{await rm(root,{recursive:true,force:true});}
});
