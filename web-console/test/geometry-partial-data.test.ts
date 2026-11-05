import {expect,it} from 'vitest';
import {partialBudget} from '../src/app/components/documents/geometry/partial-data';
import type {GeometryModel} from '../src/app/components/documents/geometry/model';
import {GEOMETRY_LIMITS as L} from '../src/app/components/documents/geometry/limits';
const model=(groups:number):GeometryModel=>({root:{name:'',meshes:[0],children:[]},meshes:[{name:'',positions:new Float32Array([0,0,0,1,0,0,0,1,0]),indices:new Uint32Array(Array.from({length:groups*3},(_,i)=>i%3)),groups:Array.from({length:groups},(_,i)=>({start:i*3,count:3,color:[i/32,.4,.6]}))}]});
it('bounds materials across the entire prefix rather than separately per completed mesh',()=>{
  const budget=partialBudget();for(let i=0;i<15;i++)budget.admit(model(15));
  expect(()=>budget.admit(model(16))).toThrow('limit');expect(budget.remaining.updates).toBe(1);
});
it('rejects a small view that retains an oversized transferred backing buffer',()=>{
  const value=model(1);value.meshes[0].positions=new Float32Array(new ArrayBuffer(L.partialBytes+4),0,9);
  expect(()=>partialBudget().admit(value)).toThrow('invalid');
});
it('charges the normal stream that the receiver must generate for a normal-free fragment',()=>{
  const value=model(1);value.meshes[0].positions=new Float32Array(L.partialBytes*3/16);
  expect(()=>partialBudget().admit(value)).toThrow('limit');
});
