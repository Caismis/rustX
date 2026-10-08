// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { randomUUID } from 'node:crypto';
import { WorkspaceTerminals } from '../host/workbench';
import type { TerminalProcess } from '../host/terminal-process';
function gate() { let resolve!: () => void, reject!: (error: Error) => void; const promise = new Promise<void>((yes, no) => { resolve = yes; reject = no; }); void promise.catch(() => {}); return { promise, resolve, reject }; }
function fixture() {
  const units: { ready: ReturnType<typeof gate>; done: ReturnType<typeof gate>; stop: ReturnType<typeof vi.fn>; write: ReturnType<typeof vi.fn> }[] = [];
  const spawn = vi.fn((): TerminalProcess => {
    const ready=gate(),done=gate(),stop=vi.fn(()=>done.promise),write=vi.fn();units.push({ready,done,stop,write});
    return { ready:ready.promise,done:done.promise,stop,write,resize:vi.fn() };
  });
  const terminals=new WorkspaceTerminals('/supervisor',spawn),owner=JSON.stringify(['A','node-A']);
  const create=(id=randomUUID())=>({id,work:terminals.request(owner,'/tmp',{kind:'create',id,shell:'/bin/sh'})});
  return {terminals,owner,units,spawn,create};
}
it('creation reserves exactly one unit; duplicate admission waits on the same unit',async()=>{
 const f=fixture(),a=f.create(),b=f.create(a.id);expect(f.spawn).toHaveBeenCalledOnce();
 f.units[0].ready.resolve();await Promise.all([a.work,b.work]);
 const closing=f.terminals.close();f.units[0].done.resolve();await closing;
});
it.each(['session retirement','Host shutdown'] as const)('%s fences creation before readiness and waits for settlement',async reason=>{
 const f=fixture(),a=f.create(),duplicate=f.create(a.id);const creation=expect(a.work).rejects.toThrow('retired');const duplicateCreation=expect(duplicate.work).rejects.toThrow('retired');
 const closing=reason==='Host shutdown'?f.terminals.close():f.terminals.retireSession('A');
 let settled=false;void closing.then(()=>{settled=true;});f.units[0].ready.resolve();await Promise.resolve();expect(settled).toBe(false);
 expect(f.units[0].stop).toHaveBeenCalledOnce();f.units[0].done.resolve();await closing;await creation;await duplicateCreation;
});
it('close, natural exit, repeated close and Host shutdown share one terminal settlement',async()=>{
 const f=fixture(),a=f.create();f.units[0].ready.resolve();await a.work;
 const close=f.terminals.request(f.owner,'',{kind:'close',id:a.id});
 const again=f.terminals.request(f.owner,'',{kind:'close',id:a.id});const shutdown=f.terminals.close();
 expect(f.units[0].stop).toHaveBeenCalledOnce();f.units[0].done.resolve();await Promise.all([close,again,shutdown]);
 await expect(f.terminals.request(f.owner,'',{kind:'input',id:a.id,data:'x'})).rejects.toThrow('closed');
});
it('Session retirement leaves foreign ownership alive and stale IDs cannot execute or recreate',async()=>{
 const f=fixture(),a=f.create();f.units[0].ready.resolve();await a.work;
 const foreign=JSON.stringify(['B','node-B']),id=randomUUID();const b=f.terminals.request(foreign,'/tmp',{kind:'create',id,shell:'/bin/sh'});f.units[1].ready.resolve();await b;
 const retired=f.terminals.retireSession('A');f.units[0].done.resolve();await retired;
 expect(f.units[1].stop).not.toHaveBeenCalled();
 await expect(f.terminals.request(f.owner,'',{kind:'input',id:a.id,data:'x'})).rejects.toThrow('belong');
 await expect(f.create(a.id).work).rejects.toThrow('settled');
 await expect(f.terminals.request(foreign,'',{kind:'close',id:a.id})).rejects.toThrow('belong');
 await f.terminals.request(f.owner,'',{kind:'close',id:a.id});
 const close=f.terminals.close();f.units[1].done.resolve();await close;
});
it('capacity recovers only at settlement; termination failure retains capacity and rejects shutdown',async()=>{
 const f=fixture();const active=[];
 for(let i=0;i<8;i++){const a=f.create();f.units[i].ready.resolve();await a.work;active.push(a);}
 const closing=f.terminals.request(f.owner,'',{kind:'close',id:active[0].id});
 await expect(f.create().work).rejects.toThrow('maximum 8');
 f.units[0].done.resolve();await closing;
 const replacement=f.create();f.units[8].ready.resolve();await replacement.work;
 const failed=f.terminals.request(f.owner,'',{kind:'close',id:replacement.id});f.units[8].done.reject(new Error('termination denied'));
 await expect(failed).rejects.toThrow('termination denied');await expect(f.create().work).rejects.toThrow('maximum 8');
 const shutdown=f.terminals.close();for(const unit of f.units.slice(1,8))unit.done.resolve();await expect(shutdown).rejects.toThrow('settlement failed');
});

it('failed admission releases capacity only after the process owner settles', async () => {
 const f=fixture(), a=f.create();
 const failed=expect(a.work).rejects.toThrow('spawn denied');
 f.units[0].ready.reject(new Error('spawn denied'));
 await Promise.resolve();
 expect(f.units[0].stop).toHaveBeenCalledOnce();
 expect((await f.terminals.request(f.owner, '', {kind:'terminals'})).terminals).toHaveLength(1);
 f.units[0].done.resolve();await failed;
 expect((await f.terminals.request(f.owner, '', {kind:'terminals'})).terminals).toHaveLength(0);
 await f.terminals.close();
});
