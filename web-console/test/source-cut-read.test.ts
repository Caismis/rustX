import {expect,it,vi} from 'vitest';
import {readSourceSettings,sourceObservation} from '../../protocol/app-server/source-observation';
import type {BoundedSourceSettings,SourceFieldRef,SourceValueChunk} from '../../protocol/app-server/v78';
import {cfg3Source} from './cfg3-data';

const expired=()=>Object.assign(new Error('expired'),{error:{data:{kind:'read_cut_expired'}}});
function cut(revision:string):BoundedSourceSettings {
 const source=cfg3Source();source.target={kind:'user'};source.user.revision=revision;
 return {target:source.target,revision,inline:null,collections:[],fields:Object.entries(source).map(([name,value])=>name==='user'
  ?{name,reference:{revision,field:'user',item_index:null,byte_length:Buffer.byteLength(JSON.stringify(value))}}
  :{name,value}) as BoundedSourceSettings['fields']};
}
function value(reference:SourceFieldRef,offset:number):SourceValueChunk {
 const source=cfg3Source();source.user.revision=reference.revision;
 const bytes=Buffer.from(JSON.stringify(source.user));
 return {reference,offset,end_offset:bytes.length,data_base64:bytes.toString('base64'),next_offset:null};
}
it('restarts an expired configuration cut and publishes only the complete new revision',async()=>{
 const capture=vi.fn().mockResolvedValueOnce(cut('before')).mockResolvedValue(cut('after'));
 const read=vi.fn(async(reference:SourceFieldRef,offset:number)=>{if(reference.revision==='before')throw expired();return value(reference,offset);});
 const source=await readSourceSettings(capture,read,()=>true);
 expect(source.user.revision).toBe('after');expect(sourceObservation(source)?.revision).toBe('after');
 expect(capture).toHaveBeenCalledTimes(2);expect(read.mock.calls.map(([ref])=>ref.revision)).toEqual(['before','after']);
});
it('continuous changes fail within a finite read budget without publishing partial data',async()=>{
 const capture=vi.fn(async()=>cut('changing')),read=vi.fn(async()=>{throw expired();});
 await expect(readSourceSettings(capture,read,()=>true)).rejects.toThrow('expired');
 expect(capture).toHaveBeenCalledTimes(3);
});
it('native rejection is reported without retrying the read',async()=>{
 const capture=vi.fn(async()=>cut('current')),read=vi.fn(async()=>{throw new Error('permission denied');});
 await expect(readSourceSettings(capture,read,()=>true)).rejects.toThrow('permission denied');
 expect(capture).toHaveBeenCalledTimes(1);
});
it('a retired configuration lifetime cannot restart an expired read',async()=>{
 let current=true;
 const capture=vi.fn(async()=>cut('retired')),read=vi.fn(async()=>{current=false;throw expired();});
 await expect(readSourceSettings(capture,read,()=>current)).rejects.toThrow('expired');
 expect(capture).toHaveBeenCalledTimes(1);
});
