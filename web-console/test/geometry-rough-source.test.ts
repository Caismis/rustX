// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { isStepSource, stepPointSamples, type GeometryRoughPreview } from '../src/client/geometry-rough';
import { readGeometrySource } from '../src/client/geometry-source';
import type { FileReadOptions } from '../shared/file-preview';
import { FILE_PREVIEW_LIMITS as L } from '../shared/file-preview';
const text="ISO-10303-21;\nHEADER; ENDSEC; DATA;\n#1=CARTESIAN_POINT('',(0.,0.,0.));\n#2=CARTESIAN_POINT('',(1.,0.,0.));\n#3=CARTESIAN_POINT('',(0.,1.,0.));\n#4=CARTESIAN_POINT('',(0.,0.,1.));\nENDSEC;END-ISO-10303-21;";
it('extracts bounded finite source coordinates without accepting other formats',()=>{
  const bytes=new TextEncoder().encode(text);expect(isStepSource(bytes)).toBe(true);expect(stepPointSamples(bytes)).toEqual([0,0,0,1,0,0,0,1,0,0,0,1]);
  expect(isStepSource(new TextEncoder().encode('solid triangle'))).toBe(false);
  expect(stepPointSamples(new TextEncoder().encode(text.replace('1.,0.,0.','1.E999,0.,0.')))).toHaveLength(9);
});
it('publishes only matching-version rough data before complete bytes and keeps all final source bytes',async()=>{
  const source=new Uint8Array(L.pageBytes+128);source.fill(32);source.set(new TextEncoder().encode(text));let finished=false;
  const rough=vi.fn((_value:GeometryRoughPreview)=>expect(finished).toBe(false));
  const read=vi.fn(async(options:FileReadOptions)=>{if(options.kind!=='bytes'||!options.range)throw Error('range');const{offset,length}=options.range;const data=source.slice(offset,offset+length);return{data,bytes:source.length,offset,version:'one',eof:offset+data.length===source.length};});
  const result=await readGeometrySource(read,new AbortController().signal,rough);finished=true;
  expect(rough).toHaveBeenCalledOnce();expect(rough.mock.calls[0][0].version).toBe('one');expect(result.data.length).toBe(source.length);expect(result.data.every((value,index)=>value===source[index])).toBe(true);expect(read).toHaveBeenCalledTimes(6);
});
it('rejects later source changes without publishing mixed-version rough geometry',async()=>{
  const source=new Uint8Array(L.pageBytes+128);source.fill(32);source.set(new TextEncoder().encode(text));const rough=vi.fn();
  await expect(readGeometrySource(async(options)=>{if(options.kind!=='bytes'||!options.range)throw Error('range');const{offset,length}=options.range,data=source.slice(offset,offset+length);return{data,bytes:source.length,offset,version:offset?'two':'one',eof:offset+data.length===source.length};},new AbortController().signal,rough)).rejects.toMatchObject({kind:'source_changed'});
  expect(rough).toHaveBeenCalledOnce();expect(rough.mock.calls[0][0].version).toBe('one');expect([...rough.mock.calls[0][0].positions]).toEqual([0,0,0,1,0,0,0,1,0,0,0,1]);
});
it('releases a paused paint yield on cancellation without starting another physical read',async()=>{
  vi.stubGlobal('requestAnimationFrame',vi.fn(()=>1));vi.stubGlobal('cancelAnimationFrame',vi.fn());const controller=new AbortController(),data=new TextEncoder().encode(text),rough=vi.fn();
  const read=vi.fn(async()=>({data,bytes:data.length,offset:0,version:'one',eof:true}));
  try{const pending=readGeometrySource(read,controller.signal,rough);await vi.waitFor(()=>expect(rough).toHaveBeenCalledOnce());const rejection=expect(pending).rejects.toMatchObject({name:'AbortError'});controller.abort();await rejection;expect(read).toHaveBeenCalledOnce();}finally{vi.unstubAllGlobals();}
});
