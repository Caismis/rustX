import type {CheckpointPage,TraceInspectionHead,TraceInspectionMessage,TraceKind} from './v57.ts';

const object=(value:unknown):value is Record<string,unknown>=>!!value&&typeof value==='object'&&!Array.isArray(value);
const userSource=(value:unknown)=>value==='human'||value==='fleet'||value==='external_system'||value==='runtime'||object(value)&&(object(value.agent)&&typeof value.agent.agent_id==='string'||object(value.extension)&&typeof value.extension.contributor==='string');
const coordinate=(value:unknown)=>typeof value==='number'&&Number.isSafeInteger(value)&&value>=0;
const count=(value:unknown):value is string=>typeof value==='string'&&/^(0|[1-9][0-9]{0,19})$/.test(value)&&BigInt(value)<=18446744073709551615n;
const contentKinds={attempt:'lifecycle',step:'lifecycle',request:'request',assistant:'messages',user:'messages',context:'messages',tool:'tool',compaction:'lifecycle',background:'lifecycle',subagent:'lifecycle',workflow:'lifecycle',interaction:'lifecycle'} satisfies Record<TraceKind,string>;

/** Decode only the complete native finite head, never a partial original TraceDetail. */
export function traceInspectionHead(page:CheckpointPage,recordId:string):TraceInspectionHead|undefined {
 const item=page.items[0],value=item?.value;
 if(!item?.deferred)return;
 // Request's indexed detail is a separate current native family.
 if(object(value)&&value.kind==='request')return;
 const sequence=/^trace:([1-9][0-9]*)$/.exec(recordId)?.[1];
 if(!sequence||page.domain!==`trace.records.${sequence}.detail`||page.total!=='1'||page.cursor!==page.read_cut.revision||page.items.length!==1||page.next_offset!=null||item.offset!=='0'||item.identity!==recordId||item.body_complete!==false
  ||!object(value)||value.id!==recordId||typeof value.kind!=='string'||!(value.kind in contentKinds)||value.kind==='request'||!object(value.record)||!object(value.content))throw Error('Original Trace inspection head changed.');
 const record=value.record,content=value.content;
 if(record.id!==recordId||record.kind!==value.kind||typeof record.position!=='string'||!object(record.location)||!object(record.timing)||typeof record.state!=='string'||typeof record.has_detail!=='boolean'||typeof record.truncated!=='boolean'
  ||!Array.isArray(record.fields)||!Array.isArray(record.collections)||!Array.isArray(record.calls)||!Array.isArray(record.attachments)
  ||content.type!==contentKinds[value.kind as TraceKind])throw Error('Original Trace inspection facts changed.');
 if(content.type==='tool'){
  if(!object(record.tool)||!Array.isArray(record.tool.fields)||!object(content.definition))throw Error('Original Tool inspection head changed.');
  const definition=content.definition;
  if(definition.type==='available'){
   const field=definition.field;
   if(!object(field)||field.type!=='value'||field.field!=='definition'||typeof field.domain!=='string'||!/^trace\.records\.[1-9][0-9]*\.detail\.request\.tools$/.test(field.domain)||!coordinate(field.item_offset)||!Array.isArray(field.path)||field.path.length!==0||!count(field.encoded_bytes))throw Error('Original Tool definition selector changed.');
  }else if(definition.type!=='absent'&&definition.type!=='unavailable')throw Error('Original Tool definition evidence changed.');
 }else if(content.type==='messages'){
  const collection=content.page;
  if(!object(collection)||collection.domain!==`${page.domain}.messages`||!count(collection.total)||!Array.isArray(collection.items)||collection.items.length>32
   ||collection.next_offset!=null&&!coordinate(collection.next_offset))throw Error('Original Trace message collection changed.');
  const end=collection.items.length;
  if(BigInt(end)>BigInt(collection.total)||(collection.next_offset??null)!==(BigInt(end)<BigInt(collection.total)?end:null))throw Error('Original Trace message collection did not advance.');
  collection.items.forEach(traceInspectionMessage);
 }
 // The wire value is the complete generated head family. Original unbounded
 // bodies have a different type and can only be opened through their sources.
 return value as unknown as TraceInspectionHead;
}

/** A selected canonical head keeps its original identity, sources and continuations. */
export function traceInspectionMessage(value:unknown):TraceInspectionMessage {
 if(!object(value)||!object(value.locator)||value.locator.type!=='by_ledger_position'||!coordinate(value.locator.position)||value.locator.position===0
  ||value.identity!=null&&typeof value.identity!=='string'||typeof value.ordinary_input!=='boolean'||value.source!=null&&!userSource(value.source)
  ||value.context_kind!=null&&typeof value.context_kind!=='string'||!Array.isArray(value.fields)||!Array.isArray(value.collections)||!Array.isArray(value.attachments)
  ||value.preview!=null&&(!object(value.preview)||typeof value.preview.text!=='string'||typeof value.preview.truncated!=='boolean'))throw Error('Original Trace canonical head changed.');
 return value as unknown as TraceInspectionMessage;
}

/** Native canonical Tool pages use original block coordinates, independently of Tool count. */
export function validateCanonicalInspectionPage(page:CheckpointPage,offset:number,total:string,advertisedContinuation=false):void {
 if(page.total!==total||!count(page.total)||page.items.length>32||!coordinate(offset)||page.items.some(item=>!count(item.offset))||page.next_offset!=null&&!count(page.next_offset))throw new Error('Original canonical collection changed.');
 if(advertisedContinuation&&!page.items.length)throw new Error('Advertised original canonical continuation did not advance.');
 const extent=BigInt(page.total),positions=page.items.map(item=>Number(item.offset)),next=page.next_offset==null?undefined:Number(page.next_offset);
 if(positions.some((at,index)=>!coordinate(at)||String(at)!==page.items[index]!.offset||at<offset||index>0&&at<=positions[index-1]!)||next!==undefined&&(!coordinate(next)||String(next)!==page.next_offset||next<offset||!positions.length||next<=positions.at(-1)!))throw new Error('Original canonical collection coordinates changed.');
 if(page.domain.endsWith('.tools')){
  if(BigInt(positions.length)>extent||offset===0&&BigInt(positions.length)<extent&&next===undefined)throw new Error('Original canonical Tool collection failed to advance.');
  return;
 }
 const end=offset+positions.length;
 if(!coordinate(end)||positions.some((at,index)=>at!==offset+index)||BigInt(end)>extent||(page.next_offset??null)!==(BigInt(end)<extent?String(end):null)||BigInt(end)<extent&&!positions.length)throw new Error('Original canonical collection failed to advance.');
}
