import type {CheckpointPage,TraceInspectionHead} from '../../../../protocol/app-server/v50';
import {traceFields} from '../../../../protocol/app-server/trace-identity';
import type {TraceFieldReader} from '../../client/trace';
import {useTranslation} from '../../locale/react';
import {TraceFields} from './TraceFields';
import {TraceCollections} from './TraceCollections';
import css from './Trajectory.module.css';

/** Finite native previews plus explicit original fields, never an assembled Tool body. */
export function TraceToolFields({head,page,reader,facet,preview=false}:{head:TraceInspectionHead;page:CheckpointPage;reader?:TraceFieldReader;facet:'input'|'output'|'schema';preview?:boolean}) {
 const tx=useTranslation();
 if(head.content.type!=='tool')return null;
 if(facet==='schema'){
  const definition=head.content.definition;
  return definition.type==='available'
   ? <TraceFields fields={[definition.field]} cut={page.read_cut} reader={reader} owned={()=>true}/>
   : <p role="status" className={css.noPayload}>{tx(definition.type==='absent'?'trajectory:record.schema-absent':'trajectory:record.schema-unavailable')}</p>;
 }
 const fields=traceFields(head.record).filter(field=>facet==='input'?field.field==='arguments':field.field==='result'||field.field==='failure');
 const text=facet==='input'?head.record.tool?.arguments:head.record.preview??head.record.tool?.detail;
 return <>
  {!fields.length && text && <pre className={preview?`${css.payload} ${css.payloadPreview}`:css.payload}>{text.text}</pre>}
  <TraceFields fields={fields} cut={page.read_cut} reader={reader} owned={()=>true}/>
  {facet==='output' && head.record.collections.map(collection => <TraceCollections key={collection.domain} collections={[collection]} domain={collection.domain} page={page} reader={reader} owned={()=>true}/>)}
  {!fields.length && !head.record.collections.length && !text && <p className={css.noPayload}>{tx(facet==='input'?'trajectory:record.no-payload':'trajectory:record.no-result')}</p>}
 </>;
}
