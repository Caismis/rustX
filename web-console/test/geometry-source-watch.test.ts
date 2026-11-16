import {expect,it,vi} from 'vitest';import {PreviewContentOwner} from '../src/client/preview-content';
it('authorizes fresh geometry bytes before watch setup while retaining its readiness and loss semantics',async()=>{
  let finish!:(value:{data:Uint8Array<ArrayBuffer>;version:string})=>void,ready!:(value:any)=>void,end!:()=>void;
  const done=new Promise<void>(resolve=>{end=resolve;}),close=vi.fn(async()=>{end();});
  const observe=vi.fn(()=>new Promise<any>(resolve=>{ready=resolve;}));
  const owner=new PreviewContentOwner({current:()=>true,immutable:false,observe,probe:async()=>'one',page:async()=>{throw Error('page');},document:async()=>{throw Error('document');},bytes:()=>{return new Promise(resolve=>{finish=resolve;});}});
  try{owner.activate();owner.ensure('geometry');expect(owner.getSnapshot().data).toBeUndefined();expect(observe).not.toHaveBeenCalled();
    finish({data:new Uint8Array([1]),version:'one'});await vi.waitFor(()=>expect(observe).toHaveBeenCalledOnce());expect(owner.getSnapshot().data).toBeDefined();expect(owner.getSnapshot().loading).toBe(true);
    ready({version:'one',done,close});await vi.waitFor(()=>expect(owner.getSnapshot().loading).toBe(false));end();await vi.waitFor(()=>expect(owner.getSnapshot().data).toBeUndefined());
  }finally{owner.dispose();}
});
