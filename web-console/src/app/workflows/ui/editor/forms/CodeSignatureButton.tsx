import { useContext, useEffect, useRef, useState } from 'react';
import { useTranslation } from '../../../../../locale/react';
import { IconRefreshOutline16 } from '../../../../../presentation/primitives/icons';
import { WorkflowAnalysisSession, consumerAddress } from '../../../document/analysis';
import { nodeField, setField } from '../../../document/fields';
import { blockAt, nodeAt } from '../../../document/model';
import { DraftOwner } from './drafts';
import type { PanelProps } from '../panels/shared';
import css from '../Editor.module.css';

/** One explicit signature edit over native type analysis of the current document. */
export function CodeSignatureButton(props:PanelProps&{onError:(failed:boolean)=>void}) {
  const tx=useTranslation(),owner=useContext(DraftOwner)!,[busy,setBusy]=useState(false),sequence=useRef(0);
  useEffect(()=>()=>{sequence.current++;},[]);
  const sync=async()=>{
    owner.commits.flush();const context=owner.editor.getSnapshot().context,document=context.document;if(!document)return;
    const current=()=>{const latest=owner.editor.getSnapshot().context;return latest.port===context.port&&latest.document===document&&latest.generation===context.generation&&latest.drafts===context.drafts;};
    const node=nodeAt(blockAt(document,props.block)!,props.id);if(node?.type!=='code')return;
    const request=++sequence.current;setBusy(true);props.onError(false);
    try {
      const inputs=await Promise.all(Object.entries(node.variables).map(async([name,value])=>{
        if(node.code_language==='javascript'||value.type!=='reference'||!value.path.length)return {name};
        const consumer=consumerAddress(document,nodeField(props.block,props.id,['variables',{key:name}]))!;
        const input={port:context.port,target:context.target,document,generation:context.generation,query:{consumer,search:'',reference:value.path,limit:1}};
        const report=await new WorkflowAnalysisSession().request(input,()=>current()?input:undefined);
        if(!report)throw new Error('stale analysis');
        return {name,schema:report.analysis.selected?.schema};
      }));
      const {synchronizeMain}=await import('./code-syntax');
      if(!current()||request!==sequence.current)return;
      const code=synchronizeMain(node.code_language,node.code,inputs);
      if(code===undefined){props.onError(true);return;}
      if(code!==node.code)props.edit(latest=>latest===document?setField(latest,nodeField(props.block,props.id,['code']),code):latest);
    }catch{if(current()&&request===sequence.current)props.onError(true);}
    finally{if(request===sequence.current)setBusy(false);}
  };
  return <button type="button" className={css.iconButton} disabled={busy} aria-label={tx('workflows:code.sync-signature')} title={tx('workflows:code.sync-signature')} aria-busy={busy} onClick={()=>void sync()}><IconRefreshOutline16/></button>;
}
