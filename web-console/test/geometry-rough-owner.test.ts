import { expect,it,vi } from 'vitest';
import { PreviewContentOwner } from '../src/client/preview-content';
import type { GeometryRoughPreview } from '../src/client/geometry-rough';
it('retires partial rough geometry and fences late sampling before a physical read settles',async()=>{
  let publish!:(value:GeometryRoughPreview)=>void,finish!:(value:{data:Uint8Array<ArrayBuffer>;version:string})=>void;
  const owner=new PreviewContentOwner({current:()=>true,immutable:true,page:async()=>{throw Error('page');},document:async()=>{throw Error('document');},probe:async()=>'one',bytes:(_signal,_mode,rough)=>{publish=rough!;return new Promise(resolve=>{finish=resolve;});}});
  try{owner.activate();owner.ensure('geometry');publish({version:'one',positions:new Float32Array([0,0,0,1,0,0,0,1,0])});expect(owner.getSnapshot().data).toBeUndefined();expect(owner.getSnapshot().geometryPreview?.version).toBe('one');
    owner.deactivate();expect(owner.getSnapshot().geometryPreview).toBeUndefined();publish({version:'one',positions:new Float32Array(9)});expect(owner.getSnapshot().geometryPreview).toBeUndefined();finish({data:new Uint8Array([1]),version:'one'});await vi.waitFor(()=>expect(owner.getSnapshot().data).toBeUndefined());
  }finally{owner.dispose();}
});
it('does not attach rough geometry to a different completed source version',async()=>{
  const owner=new PreviewContentOwner({current:()=>true,immutable:true,page:async()=>{throw Error('page');},document:async()=>{throw Error('document');},probe:async()=>'one',bytes:async(_signal,_mode,rough)=>{rough?.({version:'one',positions:new Float32Array(9)});return{data:new Uint8Array([1]),version:'two'};}});
  try{owner.activate();owner.ensure('geometry');await vi.waitFor(()=>expect(owner.getSnapshot().data).toBeDefined());expect(owner.getSnapshot().geometryPreview).toBeUndefined();expect(owner.getSnapshot().version).toBe('two');}finally{owner.dispose();}
});
