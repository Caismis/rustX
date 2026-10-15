import {useState} from 'react';
import type {CheckpointPage,TraceInspectionHead} from '../../../../protocol/app-server/v48';
import {traceFields} from '../../../../protocol/app-server/trace-identity';
import type {TraceFieldReader} from '../../client/trace';
import {useTranslation} from '../../locale/react';
import {TraceFields} from './TraceFields';
import {TraceCollections} from './TraceCollections';
import css from './Trajectory.module.css';

/** Finite native previews plus explicit original fields, never an assembled Tool body. */
export function TraceToolFields({head,page,reader,facet,preview=false}:{head:TraceInspectionHead;page:CheckpointPage;reader?:TraceFieldReader;facet:'input'|'output'|'schema';preview?:boolean}) {
 const tx=useTranslation(),[source,setSource]=useState('field:0');
 if(head.content.type!=='tool')return null;
 if(facet==='schema'){
  const definition=head.content.definition;
  return definition.type==='available'
   ? <TraceFields fields={[definition.field]} cut={page.read_cut} reader={reader} owned={()=>true}/>
   : <p role="status" className={css.noPayload}>{tx(definition.type==='absent'?'trajectory:record.schema-absent':'trajectory:record.schema-unavailable')}</p>;
 }
 const fields=traceFields(head.record).filter(field=>facet==='input'?field.field==='arguments':field.field==='result'||field.field==='failure');
 const collection=facet==='output'?(head.record.collections.find(value=>value.domain===source)??(!fields.length?head.record.collections[0]:undefined)):undefined,index=/^field:(0|[1-9][0-9]*)$/.test(source)?Number(source.slice(6)):0,field=fields[index]??fields[0];
 const text=facet==='input'?head.record.tool?.arguments:head.record.preview??head.record.tool?.detail;
 return <>
  {text&&<pre className={preview?`${css.payload} ${css.payloadPreview}`:css.payload}>{text.text}</pre>}
  {text?.truncated&&<p role="status">{tx('common:activity.content-deferred')}</p>}
  {facet==='output'&&(fields.length+head.record.collections.length>1)&&<label>{tx('common:activity.collection')} <select value={collection?.domain??`field:${fields[index]?index:0}`} onChange={event=>setSource(event.target.value)}>{fields.map((field,index)=><option key={`field:${index}`} value={`field:${index}`}>{field.field} · {field.type}</option>)}{head.record.collections.map(value=><option key={value.domain} value={value.domain}>{value.domain} ({value.total})</option>)}</select></label>}
  {collection?<TraceCollections collections={head.record.collections} domain={collection.domain} page={page} reader={reader} owned={()=>true}/>:fields.length?<TraceFields key={facet==='output'?String(index):'input'} fields={facet==='output'&&field?[field]:fields} cut={page.read_cut} reader={reader} owned={()=>true}/>:!text&&<p className={css.noPayload}>{tx(facet==='input'?'trajectory:record.no-payload':'trajectory:record.no-result')}</p>}
 </>;
}
