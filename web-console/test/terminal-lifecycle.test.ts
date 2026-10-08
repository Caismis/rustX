// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { randomUUID } from 'node:crypto';
import { WorkspaceTerminals } from '../host/workbench';
import type { TerminalProcess } from '../host/terminal-process';
function gate() { let resolve!: () => void, reject!: (error: Error) => void; const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; }); void promise.catch(() => {}); return { promise, resolve, reject }; }
function fixture() {
  const units: { ready: ReturnType<typeof gate>; done: ReturnType<typeof gate>; stop: ReturnType<typeof vi.fn>; write: ReturnType<typeof vi.fn>; output: (data: string) => void }[] = [];
  const spawn = vi.fn((_binary: string, _shell: string, _cwd: string, output: (data: string) => void): TerminalProcess => {
    const ready=gate(),done=gate(),stop=vi.fn(()=>done.promise),write=vi.fn();units.push({ready,done,stop,write,output});
    return { ready:ready.promise,done:done.promise,stop,write,resize:vi.fn() };
  });
  const terminals=new WorkspaceTerminals('/supervisor',spawn),owner={session_id:'A',active_node:'node-A'};
  const create=(id=randomUUID())=>({id,work:terminals.request(owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'create',id,shell:'/bin/sh'})});
  return {terminals,owner,units,spawn,create};
}
it('creation reserves exactly one unit; duplicate admission waits on the same unit',async()=>{
 const f=fixture(),a=f.create(),b=f.create(a.id);await Promise.resolve();expect(f.spawn).toHaveBeenCalledOnce();
 f.units[0].ready.resolve();await Promise.all([a.work,b.work]);
 const closing=f.terminals.close();f.units[0].done.resolve();await closing;
});
it.each(['session retirement','Host shutdown'] as const)('%s fences creation before readiness and waits for settlement',async reason=>{
 const f=fixture(),a=f.create(),duplicate=f.create(a.id);await Promise.resolve();const creation=expect(a.work).rejects.toThrow(reason==='Host shutdown'?'closed':'retired');const duplicateCreation=expect(duplicate.work).rejects.toThrow(reason==='Host shutdown'?'closed':'retired');
 const closing=reason==='Host shutdown'?f.terminals.close():f.terminals.retireOwnership('A','1');
 let settled=false;void closing.then(()=>{settled=true;});f.units[0].ready.resolve();await Promise.resolve();expect(settled).toBe(false);
 expect(f.units[0].stop).toHaveBeenCalledOnce();f.units[0].done.resolve();await closing;await creation;await duplicateCreation;
});
it('close, natural exit, repeated close and Host shutdown share one terminal settlement',async()=>{
 const f=fixture(),a=f.create();await Promise.resolve();f.units[0].ready.resolve();await a.work;
 const close=f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'close',id:a.id});
 const again=f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'close',id:a.id});const shutdown=f.terminals.close();
 expect(f.units[0].stop).toHaveBeenCalledOnce();f.units[0].done.resolve();await Promise.all([close,again,shutdown]);
 await expect(f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'input',id:a.id,data:'x'})).rejects.toThrow('closed');
});
it('Session retirement leaves foreign ownership alive and stale IDs cannot execute or recreate',async()=>{
 const f=fixture(),a=f.create();await Promise.resolve();f.units[0].ready.resolve();await a.work;
 const foreign={session_id:'B',active_node:'node-B'},id=randomUUID();const b=f.terminals.request(foreign,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'create',id,shell:'/bin/sh'});await Promise.resolve();f.units[1].ready.resolve();await b;
 const retired=f.terminals.retireOwnership('A','1');f.units[0].done.resolve();await retired;
 expect(f.units[1].stop).not.toHaveBeenCalled();
 await expect(f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'input',id:a.id,data:'x'})).rejects.toThrow('belong');
 await expect(f.create(a.id).work).rejects.toThrow('settled');
 await expect(f.terminals.request(foreign,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'close',id:a.id})).rejects.toThrow('belong');
 await f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'close',id:a.id});
 const close=f.terminals.close();f.units[1].done.resolve();await close;
});
it('capacity recovers only at settlement; termination failure retains capacity and rejects shutdown',async()=>{
 const f=fixture();const active=[];
 for(let i=0;i<8;i++){const a=f.create();await Promise.resolve();f.units[i].ready.resolve();await a.work;active.push(a);}
 const closing=f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'close',id:active[0].id});
 await expect(f.create().work).rejects.toThrow('maximum 8');
 f.units[0].done.resolve();await closing;
 const replacement=f.create();await Promise.resolve();f.units[8].ready.resolve();await replacement.work;
 const failed=f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'close',id:replacement.id});f.units[8].done.reject(new Error('termination denied'));
 await expect(failed).rejects.toThrow('termination denied');await expect(f.create().work).rejects.toThrow('maximum 8');
 const shutdown=f.terminals.close();for(const unit of f.units.slice(1,8))unit.done.resolve();await expect(shutdown).rejects.toThrow('settlement failed');
});

it('failed admission releases capacity only after the process owner settles', async () => {
 const f=fixture(), a=f.create();await Promise.resolve();
 const failed=expect(a.work).rejects.toThrow('spawn denied');
 f.units[0].ready.reject(new Error('spawn denied'));
 await Promise.resolve();
 expect(f.units[0].stop).toHaveBeenCalledOnce();
 expect((await f.terminals.request(f.owner, async()=>({cwd:'/tmp',ownershipGeneration:'1'}), {kind:'terminals'})).terminals).toHaveLength(1);
 f.units[0].done.resolve();await failed;
 expect((await f.terminals.request(f.owner, async()=>({cwd:'/tmp',ownershipGeneration:'1'}), {kind:'terminals'})).terminals).toHaveLength(0);
 await f.terminals.close();
});

it.each(['B', 'node-A'])('delayed retirement of A1 preserves successor %s (including ABA)', async node => {
  const f = fixture(), first = f.create();
  await Promise.resolve(); f.units[0].ready.resolve(); await first.work;
  const successor = { session_id: 'A', active_node: node }, id = randomUUID();
  const read = async () => ({ cwd: '/tmp', ownershipGeneration: '3' });
  const second = f.terminals.request(successor, read, { kind: 'create', id, shell: '/bin/sh' });
  await Promise.resolve(); f.units[1].ready.resolve(); await second;
  await expect(f.terminals.request(successor, read, { kind: 'input', id: first.id, data: 'stale before delivery' })).rejects.toThrow('belong');
  const retired = f.terminals.retireOwnership('A', '1');
  expect(f.units[0].stop).toHaveBeenCalledOnce(); expect(f.units[1].stop).not.toHaveBeenCalled();
  f.units[0].done.resolve(); await retired;
  await f.terminals.request(successor, read, { kind: 'input', id, data: 'alive' });
  expect(f.units[1].write).toHaveBeenCalledExactlyOnceWith('alive');
  await expect(f.terminals.request(successor, read, { kind: 'input', id: first.id, data: 'stale' })).rejects.toThrow('belong');
  const closing = f.terminals.close(); f.units[1].done.resolve(); await closing;
});

it('coalesced and out-of-order retirement watermarks settle A and B but preserve C', async () => {
  const f = fixture();
  const owners = ['node-A', 'node-B', 'node-C'].map(active_node => ({ session_id: 'A', active_node }));
  const ids = owners.map(() => randomUUID());
  for (let i = 0; i < 3; i++) {
    const creating = f.terminals.request(owners[i], async () => ({cwd:'/tmp',ownershipGeneration:String(i+1)}), {kind:'create',id:ids[i],shell:'/bin/sh'});
    await Promise.resolve(); f.units[i].ready.resolve(); await creating;
  }
  const retirement = f.terminals.retireOwnership('A', '2');
  const repeated = f.terminals.retireOwnership('A', '1');
  const close = f.terminals.request(owners[0], async () => { throw Error('close must not read native'); }, {kind:'close',id:ids[0]});
  expect(f.units.map(unit => unit.stop.mock.calls.length)).toEqual([1,1,0]);
  f.units[0].done.resolve(); f.units[1].done.resolve(); await Promise.all([retirement,repeated,close]);
  await f.terminals.request(owners[2],async()=>({cwd:'/tmp',ownershipGeneration:'3'}),{kind:'input',id:ids[2],data:'C'});
  expect(f.units[2].write).toHaveBeenCalledExactlyOnceWith('C');
  const closing = f.terminals.close(); const again = f.terminals.retireOwnership('A','3');
  expect(f.units[2].stop).toHaveBeenCalledOnce(); f.units[2].done.resolve(); await Promise.all([closing,again]);
});

it.each(['retired read', 'successor read', 'foreign retirement', 'shutdown'] as const)('retirement before native read completion: %s', async scenario => {
  const f = fixture(), readBarrier = gate();
  const read = vi.fn(async () => { await readBarrier.promise; return {cwd:'/tmp',ownershipGeneration:scenario==='successor read'?'2':'1'}; });
  const work = f.terminals.request(f.owner, read, {kind:'create',id:randomUUID(),shell:'/bin/sh'});
  expect(read).toHaveBeenCalledOnce(); expect(f.spawn).not.toHaveBeenCalled();
  const rejected = scenario==='retired read'||scenario==='shutdown' ? expect(work).rejects.toThrow(scenario==='shutdown'?'closed':'retired') : undefined;
  if (scenario==='shutdown') await f.terminals.close();
  else await f.terminals.retireOwnership(scenario==='foreign retirement'?'foreign':'A','1');
  readBarrier.resolve();
  if (rejected) { await rejected; expect(f.spawn).not.toHaveBeenCalled(); }
  else {
    // The read completion is the controlled admission barrier, not elapsed time.
    await read.mock.results[0].value; await Promise.resolve();
    expect(f.spawn).toHaveBeenCalledOnce(); f.units[0].ready.resolve(); await work;
    const close = f.terminals.close(); f.units[0].done.resolve(); await close;
  }
});

it('retirement while capacity eviction awaits settlement cannot admit an obsolete unit', async () => {
  const f = fixture();
  for(let i=0;i<8;i++){const a=f.create();await Promise.resolve();f.units[i].ready.resolve();await a.work;}
  // A proven natural exit can be evicted; hold the stop completion separately.
  const evicted = gate(); f.units[0].stop.mockImplementation(()=>evicted.promise);
  f.units[0].done.resolve(); await f.units[0].done.promise;
  const work = f.create(); const rejected = expect(work.work).rejects.toThrow('retired');
  await Promise.resolve(); expect(f.units[0].stop).toHaveBeenCalledOnce();
  const retired = f.terminals.retireOwnership('A','1');
  evicted.resolve(); for(const unit of f.units.slice(1))unit.done.resolve();
  await retired; await rejected; expect(f.spawn).toHaveBeenCalledTimes(8);
  await f.terminals.close();
});

it('duplicate creation after a shared capacity-eviction barrier still reserves exactly one PTY', async () => {
  const f=fixture();
  for(let i=0;i<8;i++){const a=f.create();await Promise.resolve();f.units[i].ready.resolve();await a.work;}
  const eviction=gate();f.units[0].stop.mockImplementation(()=>eviction.promise);
  f.units[0].done.resolve();await f.units[0].done.promise;
  const a=f.create(),b=f.create(a.id);await Promise.resolve();
  expect(f.units[0].stop).toHaveBeenCalledOnce();
  const spawned=gate(),spawn=f.spawn.getMockImplementation()!;
  f.spawn.mockImplementation((...args)=>{const unit=spawn(...args);spawned.resolve();return unit;});
  eviction.resolve();await spawned.promise;
  f.units[8].ready.resolve();await Promise.all([a.work,b.work]);
  expect(f.spawn).toHaveBeenCalledTimes(9);
  const close=f.terminals.close();for(const unit of f.units)unit.done.resolve();await close;
});

it('retirement failure retains the unit and stays observable through repeated retirement and shutdown', async () => {
  const f=fixture(),a=f.create();await Promise.resolve();f.units[0].ready.resolve();await a.work;
  const first=f.terminals.retireOwnership('A','1'),again=f.terminals.retireOwnership('A','1');
  const failures=[expect(first).rejects.toThrow('settlement failed'),expect(again).rejects.toThrow('settlement failed')];
  f.units[0].done.reject(Error('native reaping proof unavailable'));await Promise.all(failures);
  expect(f.units[0].stop).toHaveBeenCalledOnce();
  const list=await f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'terminals'});
  expect(list.terminals).toHaveLength(1);
  await expect(f.terminals.close()).rejects.toThrow('settlement failed');
});

it('a waiting Poll revalidates retirement before publication, independently of physical settlement', async () => {
 const f=fixture(),a=f.create();await Promise.resolve();f.units[0].ready.resolve();await a.work;
 const entered=gate(),controller=new AbortController();
 const listen=controller.signal.addEventListener.bind(controller.signal);
 vi.spyOn(controller.signal,'addEventListener').mockImplementation((...args)=>{listen(...args);entered.resolve();});
 const poll=f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'poll',id:a.id,cursor:0},controller.signal);
 const rejected=expect(poll).rejects.toThrow('retired');
 await entered.promise; // The long-poll wait has installed its existing abort listener.
 const retired=f.terminals.retireOwnership('A','1');
 f.units[0].output('buffered after retirement');await rejected;
 expect(f.units[0].stop).toHaveBeenCalledOnce();
 // Physical proof is still held: retirement has not released retained capacity.
 expect((await f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'1'}),{kind:'terminals'})).terminals).toHaveLength(1);
 const id=randomUUID(),create=f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'3'}),{kind:'create',id,shell:'/bin/sh'});
 await Promise.resolve();f.units[1].ready.resolve();await create;
 const successor=f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'3'}),{kind:'poll',id,cursor:0});
 await Promise.resolve();f.units[1].output('successor');expect(await successor).toMatchObject({output:'successor',exited:false});
 f.units[0].done.resolve();await retired;
 const completion=f.terminals.request(f.owner,async()=>({cwd:'/tmp',ownershipGeneration:'3'}),{kind:'poll',id,cursor:9});
 await Promise.resolve();f.units[1].done.resolve();expect(await completion).toMatchObject({output:'',exited:true});
 expect(f.units[1].stop).not.toHaveBeenCalled();await f.terminals.close();
});
