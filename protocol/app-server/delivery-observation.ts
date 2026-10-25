import type { CheckpointPage, CheckpointReadCut, CheckpointValueChunk, RuntimeClientDeliveryHead, RuntimeClientDeliveryItem, SessionFileReference } from './v57.ts';
import { canonicalLocatorKey } from './content-identity.ts';

const uint64 = (value:unknown):bigint => {
 if(typeof value!=="string"||value.length>20||!/^(0|[1-9][0-9]*)$/.test(value))throw new Error("Native delivery quantity is unavailable.");
 const integer=BigInt(value);if(integer>18446744073709551615n)throw new Error("Native delivery quantity exceeds u64.");return integer;
};
export function deliveryItem(value: unknown): RuntimeClientDeliveryItem {
 if(!value||typeof value!=='object'||Array.isArray(value))throw new Error('Native delivery item is unavailable.');
 const item=value as RuntimeClientDeliveryItem,locator=item.locator;
 if(!Number.isSafeInteger(item.index)||item.index<0||!locator||!(locator.type==='by_ledger_position'&&Number.isSafeInteger(locator.position)&&locator.position>0||locator.type==='by_message_id'&&typeof locator.message_id==='string'&&locator.message_id.length>0&&locator.message_id.length<=256&&new TextEncoder().encode(locator.message_id).length<=256)
  ||typeof item.metadata_deferred!=='boolean'||typeof item.metadata_domain!=='string'||item.metadata_domain.length>256||!item.metadata_domain||typeof item.encoded_bytes!=='string'||item.encoded_bytes.length>5||uint64(item.encoded_bytes)>65_536n)throw new Error('Native delivery item is unavailable.');
 if(item.file!=null)deliveryFile(item.file);
 return item;
}
/** Validate the finite prefix before traversing unknown selected-page values. */
export function deliveryHead(value:unknown):RuntimeClientDeliveryHead {
 if(!value||typeof value!=="object"||Array.isArray(value))throw new Error("Native delivery head is unavailable.");
 const head=value as RuntimeClientDeliveryHead;
 if(typeof head.domain!=="string"||!head.domain||head.domain.length>256||!Array.isArray(head.items)||head.items.length>8)throw new Error("Native delivery head exceeds its prefix bound.");
 const total=uint64(head.total);
 if(head.next_offset!=null&&(!Number.isSafeInteger(head.next_offset)||head.next_offset<0||BigInt(head.next_offset)>=total))throw new Error("Native delivery continuation is unavailable.");
 for(const item of head.items){deliveryItem(item);if(BigInt(item.index)>=total)throw new Error("Native delivery index exceeds its original total.");}
 return head;
}
export function deliveryFile(value:unknown):SessionFileReference {
 if(!value||typeof value!=='object'||Array.isArray(value))throw new Error('Native delivery metadata is unavailable.');
 const file=value as SessionFileReference,scope=file.scope;
 if(!scope||typeof scope.conversation_id!=='string'||typeof scope.device!=='string'||typeof scope.inode!=='string'||Object.keys(scope).some(key=>!['conversation_id','device','inode'].includes(key))
  ||typeof file.path!=='string'||typeof file.name!=='string'||typeof file.mime_type!=='string'||file.description!=null&&typeof file.description!=='string'||Object.keys(file).some(key=>!['scope','path','name','description','mime_type'].includes(key)))throw new Error('Native delivery metadata is unavailable.');
 return file;
}
export function deliveryPage(head:RuntimeClientDeliveryHead,page:CheckpointPage,cut:CheckpointReadCut,offset:number):RuntimeClientDeliveryHead {
 deliveryHead(head);
 if(page.items.length>8)throw new Error("Native delivery page exceeds its item bound.");
 const items=page.items.map(row=>{const item=deliveryItem(row.value);if(uint64(row.offset)!==BigInt(item.index))throw new Error('Native delivery index changed.');return item;});
 const end=offset+items.length,total=uint64(head.total),next=end<total?end:null;
 if(JSON.stringify(page.read_cut)!==JSON.stringify(cut)||page.domain!==head.domain||uint64(page.total)!==total||!Number.isSafeInteger(offset)||offset<0||!Number.isSafeInteger(end)||items.length>8||items.some((item,i)=>item.index!==offset+i)||BigInt(end)>total||BigInt(offset)<total&&!items.length||(page.next_offset==null?null:uint64(page.next_offset))!==(next==null?null:BigInt(next)))throw new Error('Native delivery page did not advance exactly.');
 return {...head,items:items as RuntimeClientDeliveryHead["items"],next_offset:next};
}
/** One selected original reference, capped by the native per-delivery bound, never the whole Tool result. */
export async function readDeliveryMetadata(item:RuntimeClientDeliveryItem,cut:CheckpointReadCut,read:(offset:number)=>Promise<CheckpointValueChunk>,current:()=>boolean):Promise<SessionFileReference> {
 deliveryItem(item);const end=Number(BigInt(item.encoded_bytes)),bytes=new Uint8Array(end);let offset=0;
 while(offset<end){if(!current())throw new Error('Native delivery view changed.');const chunk=await read(offset);if(!current())throw new Error('Native delivery view changed.');
  const encoded=chunk.bytes_base64;
  if(encoded.length>87_384||encoded.length%4!==0||encoded.length/4*3-(encoded.endsWith('==')?2:encoded.endsWith('=')?1:0)>65_536)throw new Error('Native delivery chunk exceeds its byte bound.');
  const data=Uint8Array.from(atob(encoded),c=>c.charCodeAt(0)),next=offset+data.length;
  if(chunk.representation!=='canonical_json_v1'||JSON.stringify(chunk.cut)!==JSON.stringify(cut)||chunk.domain!==item.metadata_domain||chunk.item_offset!==0||chunk.path.length!==0||chunk.offset!==offset||chunk.readable_end!==end||!data.length||next>end||(chunk.next_offset??null)!==(next<end?next:null))throw new Error('Native delivery metadata range changed.');
  bytes.set(data,offset);offset=next;
 }
 if(!current())throw new Error('Native delivery view changed.');return deliveryFile(JSON.parse(new TextDecoder('utf-8',{ignoreBOM:true,fatal:true}).decode(bytes)));
}
export function deliveryItemKey(item:Pick<RuntimeClientDeliveryItem,'locator'|'index'>):string{return JSON.stringify([canonicalLocatorKey(item.locator),item.index]);}
