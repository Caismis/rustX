/** Exact-cut field reads shared by native configuration projections. */
import { parseObservedValue } from './domain-observation.ts';
export async function materializeFields<T, R extends {revision:string;byte_length:number}>(view:{revision:string;inline?:T|null;fields:{name:string;value?:unknown;reference?:R|null}[]},read:(reference:R,offset:number)=>Promise<{reference:R;offset:number;end_offset:number;data_base64:string;next_offset?:number|null}>,current:()=>boolean):Promise<T> {
  if (!current()) throw new Error('Configuration view was replaced.');
  if (view.inline) return view.inline;
  if (view.fields.length > 32) throw new Error('Configuration field set exceeds its bound.');
  const fields:Record<string,unknown> = Object.create(null); let loaded=0;
  for (const field of view.fields) {
    if (!field.reference && field.value !== undefined) { loaded+=new TextEncoder().encode(JSON.stringify(field.value)).length; fields[field.name]=field.value; }
    else if (!field.reference) fields[field.name]=null;
    else {
      const reference=field.reference;
      if(reference.revision!==view.revision||!Number.isSafeInteger(reference.byte_length)||reference.byte_length<0||loaded+reference.byte_length>16*1024*1024)throw new Error('Configuration content is deferred. Read the native field pages before editing.');
      let offset=0,json=''; const decoder=new TextDecoder('utf-8',{fatal:true});
      do {
        const chunk=await read(reference,offset);
        if(!current())throw new Error('Configuration view was replaced.');
        if(JSON.stringify(chunk.reference)!==JSON.stringify(reference)||chunk.offset!==offset)throw new Error('Configuration read cut changed.');
        if(chunk.data_base64.length>87_384 || chunk.data_base64.length%4!==0 || chunk.data_base64.length/4*3-(chunk.data_base64.endsWith('==')?2:chunk.data_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
        const bytes=Uint8Array.from(atob(chunk.data_base64),value=>value.charCodeAt(0)),end=offset+bytes.length;
        if(bytes.length>65536||!Number.isSafeInteger(chunk.end_offset)||chunk.end_offset!==end||end>reference.byte_length||(offset<reference.byte_length&&!bytes.length)||(chunk.next_offset??null)!==(end<reference.byte_length?end:null))throw new Error('Configuration content failed to advance exactly.');
        json+=decoder.decode(bytes,{stream:end<reference.byte_length});offset=end;
      }while(offset<reference.byte_length);
      loaded+=reference.byte_length;fields[field.name]=parseObservedValue(json);
    }
    if(loaded>16*1024*1024)throw new Error('Configuration byte cache is full. Read one native field page.');
  }
  return fields as T;
}
