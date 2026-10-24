import type {CheckpointReadCut,CheckpointValueChunk,TraceFieldSource} from './v57.ts';

export type TraceValueField=Extract<TraceFieldSource,{type:'value'}>;
export type TraceValueSelector=Pick<TraceValueField,'domain'|'item_offset'|'path'>;
/** One explicit original byte window; the caller keeps this cut for its continuation. */
export async function readTraceFieldWindow(field:TraceValueField,cut:CheckpointReadCut,offset:number,
 read:(offset:number)=>Promise<CheckpointValueChunk>,current:()=>boolean) {
 const encoded=field.encoded_bytes;
 if(typeof encoded!=='string'||encoded.length>20||!/^(0|[1-9][0-9]*)$/.test(encoded))throw new Error('Original Trace field extent is unavailable.');
 const extent=BigInt(encoded);
 if(extent>18446744073709551615n||extent>BigInt(Number.MAX_SAFE_INTEGER))throw new Error('Original Trace field coordinates are unavailable.');
 const end=Number(extent);
 return readTraceSelectedWindow(field,cut,offset,read,current,end);
}
/** One explicit selection under an original detail page; the first source reply fixes its extent. */
export async function readTraceSelectedWindow(field:TraceValueSelector,cut:CheckpointReadCut,offset:number,
 read:(offset:number)=>Promise<CheckpointValueChunk>,current:()=>boolean,expectedEnd?:number) {
 if(typeof field.domain!=='string'||!field.domain||field.domain.length>256||!Array.isArray(field.path)||field.path.length>32||field.path.some(part=>typeof part!=='string'||part.length>256))throw new Error('Original Trace field selector is unavailable.');
 if(!Number.isSafeInteger(field.item_offset)||field.item_offset<0||!Number.isSafeInteger(offset)||offset<0
  ||expectedEnd!=null&&(!Number.isSafeInteger(expectedEnd)||expectedEnd<offset))throw new Error('Original Trace field range is unavailable.');
 if(!current())throw new Error('Trace field view was replaced.');
 const chunk=await read(offset);
 if(!current())throw new Error('Trace field view was replaced.');
 const end=chunk.readable_end;
 if(!Number.isSafeInteger(end)||end<offset||expectedEnd!=null&&end!==expectedEnd)throw new Error('Original Trace field extent changed.');
 const value=chunk.bytes_base64;
 if(value.length>87_384||value.length%4!==0||value.length/4*3-(value.endsWith('==')?2:value.endsWith('=')?1:0)>65_536)throw new Error('Trace field chunk exceeds its decoded-byte bound.');
 const bytes=Uint8Array.from(atob(value),character=>character.charCodeAt(0)),next=offset+bytes.length;
 if(JSON.stringify(chunk.cut)!==JSON.stringify(cut)||chunk.domain!==field.domain||chunk.item_offset!==field.item_offset||JSON.stringify(chunk.path)!==JSON.stringify(field.path)||chunk.offset!==offset||chunk.readable_end!==end
  ||!Number.isSafeInteger(next)||next>end||offset<end&&!bytes.length||(chunk.next_offset??null)!==(next<end?next:null)
  ||chunk.representation!=='canonical_json_v1'&&chunk.representation!=='utf8_text')throw new Error('Original Trace field range identity changed.');
 return {bytes,next:chunk.next_offset??undefined,representation:chunk.representation,readable_end:end};
}
