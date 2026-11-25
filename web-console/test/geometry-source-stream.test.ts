import { expect, it, vi } from 'vitest';
import { readGeometrySource } from '../src/client/geometry-source';
import { FILE_PREVIEW_LIMITS as L, type FileBytes } from '../shared/file-preview';
const flush = async () => { for (let turn = 0; turn < 20; turn++) await Promise.resolve(); };
it('overlaps bounded source reads with decoder ACK and holds EOF until secure close validation', async () => {
  const bytes = L.pageBytes * 5 + 13, requests: { offset: number; finish(): void }[] = [], chunks: number[] = [];
  let active = 0, maximum = 0, acknowledge!: () => void, close!: () => void, completed = false;
  const pending = readGeometrySource(async () => ({ bytes, version: 'one', read: ({ offset, length }) => new Promise<FileBytes>(resolve => {
    active++; maximum = Math.max(maximum, active);
    requests.push({ offset, finish() { active--; const data = new Uint8Array(Math.min(length, bytes - offset)); resolve({ data, offset, bytes, version: 'one', eof: offset + data.length === bytes }); } });
  }), close: () => new Promise<void>(resolve => { close = resolve; }) }), new AbortController().signal, {
    start: vi.fn(), chunk: (_data, offset) => { chunks.push(offset); return new Promise<void>(resolve => { acknowledge = resolve; }); },
  }, 'fixed2').then(() => { completed = true; });
  await flush(); requests[0].finish(); await flush();
  expect(chunks).toEqual([0]); expect(requests.map(value => value.offset)).toEqual([0, L.pageBytes, L.pageBytes * 2]); expect(maximum).toBe(2);
  requests[2].finish(); requests[1].finish(); await flush();
  expect(requests).toHaveLength(3); expect(chunks).toEqual([0]);
  acknowledge(); await flush(); expect(chunks).toEqual([0, L.pageBytes]);
  for (let index = 1; index < 6; index++) {
    for (const request of requests.filter(value => value.offset >= L.pageBytes * 3)) { if (!('done' in request)) { Object.assign(request, { done: true }); request.finish(); } }
    acknowledge(); await flush();
  }
  expect(chunks).toHaveLength(6); expect(completed).toBe(false); close(); await pending;
  expect(completed).toBe(true); expect(maximum).toBe(2);
});
it('never reports successful source completion when final identity validation fails', async () => {
  const finish = vi.fn(async () => { throw new Error('source changed at close'); });
  await expect(readGeometrySource(async () => ({ bytes: 1, version: 'one', read: async () => ({data:new Uint8Array([1]),bytes:1,version:'one',offset:0,eof:true}), close: finish }), new AbortController().signal, {start:()=>{},chunk:()=>{}})).rejects.toThrow('source changed');
});
it('counts exclusively transferred pages before the consumer detaches their buffers', async () => {
  const result = await readGeometrySource(async () => ({bytes:3,version:'one',read:async()=>({data:new Uint8Array([1,2,3]),bytes:3,version:'one',offset:0,eof:true}),close:async()=>{}}),new AbortController().signal,{start:()=>{},chunk:data=>{structuredClone(data,{transfer:[data.buffer]});}});
  expect(result).toEqual({version:'one'});
});
it('the internal adaptive experiment upgrades for read wait and downshifts when decoder ACK cost dominates', async () => {
  let now=0, chunk=0;
  const clock=vi.spyOn(performance,'now').mockImplementation(()=>now), observations: import('../src/client/geometry-source').GeometryReadObservation[]=[];
  const bytes=L.pageBytes*14;
  try{
    await readGeometrySource(async()=>({bytes,version:'one',read:async({offset,length})=>{now+=10;const data=new Uint8Array(Math.min(length,bytes-offset));return {data,bytes,version:'one',offset,eof:offset+data.length===bytes};},close:async()=>{}}),new AbortController().signal,{start:()=>{},chunk:()=>{now+=++chunk<6?1:1000;}},'adaptive',sample=>observations.push(sample));
    expect(observations.some(sample=>sample.nextWidth>sample.width)).toBe(true);
    expect(observations.some(sample=>sample.nextWidth<sample.width)).toBe(true);
    expect(observations.at(-1)?.nextWidth).toBe(1);
  }finally{clock.mockRestore();}
});
it('defaults streamed input to a one-read start, applies ACK backpressure, and ramps within two reads', async()=>{
  let now=0,acknowledge!:()=>void;
  const clock=vi.spyOn(performance,'now').mockImplementation(()=>now), bytes=L.pageBytes*10, reads:number[]=[], observations:import('../src/client/geometry-source').GeometryReadObservation[]=[];
  try{
    const pending=readGeometrySource(async()=>({bytes,version:'one',read:async({offset,length})=>{reads.push(offset);now+=10;const data=new Uint8Array(Math.min(length,bytes-offset));return {data,bytes,version:'one',offset,eof:offset+data.length===bytes};},close:async()=>{}}),new AbortController().signal,{start:()=>{},chunk:(_data,offset)=>{now++;if(!offset)return new Promise<void>(resolve=>{acknowledge=resolve;});}},undefined,sample=>observations.push(sample));
    await flush();expect(reads).toEqual([0,L.pageBytes]);acknowledge();await pending;
    expect(observations[0].width).toBe(1);expect(observations.some(sample=>sample.width===2)).toBe(true);expect(observations.every(sample=>sample.width>=1&&sample.width<=2)).toBe(true);
  }finally{clock.mockRestore();}
});
it('keeps the complete-input default at two reads without an adaptive startup ramp',async()=>{
  const clock=vi.spyOn(performance,'now').mockReturnValue(0);
  try { const bytes=L.pageBytes*3+1, requests:{offset:number;finish():void}[]=[];
  const pending=readGeometrySource(async()=>({bytes,version:'one',read:({offset,length})=>new Promise<FileBytes>(resolve=>requests.push({offset,finish(){const data=new Uint8Array(Math.min(length,bytes-offset));resolve({data,bytes,version:'one',offset,eof:offset+data.length===bytes});}})),close:async()=>{}}),new AbortController().signal);
  await flush();requests[0].finish();await flush();expect(requests.map(request=>request.offset)).toEqual([0,L.pageBytes,L.pageBytes*2]);
  requests[1].finish();requests[2].finish();await flush();requests[3].finish();expect((await pending).data.length).toBe(bytes);
  } finally { clock.mockRestore(); }
});
