import {expect,it} from 'vitest';
import {meterQueue} from '../src/app/agent/meter-queue';
const gate = () => {let release!:()=>void; const promise=new Promise<void>(r=>release=r);return {promise,release};};
it('coalesces thousands of revisions, bounds pending work and removes obsolete queued reads',async()=>{
 const schedule=meterQueue(), a=gate(), b=gate(), finished=gate(); const calls:string[]=[];
 schedule('a',async()=>{calls.push('a');await a.promise;});
 schedule('b',async()=>{calls.push('b');await b.promise;});
 for(let i=0;i<10000;i++) schedule('latest',async()=>{calls.push(`latest-${i}`);});
 const cancel=schedule('obsolete',async()=>{calls.push('obsolete');});cancel();
 schedule('end',async()=>{finished.release();});
 await Promise.resolve();expect(calls).toEqual(['a','b']);
 a.release();await finished.promise;expect(calls).toEqual(['a','b','latest-9999']);b.release();
});
it('caps pending identities at 32 and never overlaps reads of the same identity',async()=>{
 const schedule=meterQueue(), a=gate(), b=gate(), done=gate();const calls:number[]=[];
 schedule('a',async()=>{await a.promise;});schedule('b',async()=>{await b.promise;});
 for(let i=0;i<1000;i++)schedule(String(i),async()=>{calls.push(i);if(i===999)done.release();});
 a.release();await done.promise;expect(calls).toEqual(Array.from({length:32},(_,i)=>968+i));b.release();
 const held=gate(), settled=gate();let started=false;
 schedule('same',async()=>{await held.promise;});
 schedule('same',async()=>{started=true;settled.release();});
 await Promise.resolve();expect(started).toBe(false);held.release();await settled.promise;expect(started).toBe(true);
});
