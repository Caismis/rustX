import { lazy, Suspense, useState } from 'react';
import clsx from 'clsx';
import type { WorkflowVarType } from '../../../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../../../locale/react';
import { IconCheckOutline16, IconCopyOutline16, IconFullscreenOutline16, IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { Modal } from '../../../../../presentation/primitives/Modal';
import { Button } from '../../../../../presentation/primitives/Button';
import { useCopyFeedback } from '../../../../../presentation/primitives/use-copy-feedback';
import { CommandRejected, freshKey, renameCodeOutput, replaceNodeExits } from '../../../document/commands';
import { encodeField, fieldValue, setField } from '../../../document/fields';
import { blockAt, nodeAt } from '../../../document/model';
import { setOrder } from '../../../document/geometry';
import { orderedKeys } from '../../../document/presentation';
import { codeDefaults, type CodeLanguage } from '../../../document/code';
import { rewriteScopeReferences } from '../../../document/references';
import { IconPlus } from '../../icons';
import { ReferencePicker } from '../forms/ReferencePicker';
import { ConflictActions, JsonField, useFieldError } from '../forms/fields';
import { useFieldDraft } from '../forms/drafts';
import { useInspectorExpansion } from '../forms/useInspectorExpansion';
import { CodeSignatureButton } from '../forms/CodeSignatureButton';
import { ErrorStrategy, RetrySettings, RouteId } from './InferencePanels';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';
import styles from '../forms/WorkflowCode.module.css';

const WorkflowCodeView=lazy(()=>import('../forms/WorkflowCodeView'));
const TYPES:readonly WorkflowVarType[]=['string','number','boolean','object','array[string]','array[number]','array[boolean]','array[object]'];

/** Dify's Code panel, backed by one admitted Bash call in the Session workspace. */
export function CodePanel(props:PanelProps) {
  const tx=useTranslation(),{node,at,update}=useNode(props,'code'),[removing,setRemoving]=useState<string>(),[syncFailed,setSyncFailed]=useState(false);
  const orderTarget={kind:'node_output' as const,address:{block:[...props.block],node:props.id},path:[]};
  const outputs=orderedKeys(props.document,orderTarget,Object.keys(node.outputs));
  const removeOutput=(name:string)=>props.edit(document=>{
    const owner=blockAt(document,props.block),current=owner&&nodeAt(owner,props.id);
    if(current?.type!=='code')throw new CommandRejected('missing_node');
    const {[name]:_,...outputs}=current.outputs;
    const next=setField(document,at('outputs'),outputs);
    const rewritten=rewriteScopeReferences(next,props.block,path=>path[0]===props.id&&path[1]===name?[]:undefined);
    return setOrder(rewritten,orderTarget,orderedKeys(document,orderTarget,Object.keys(current.outputs)).filter(key=>key!==name));
  });
  const requestRemove=(name:string)=>{
    let used=false;rewriteScopeReferences(props.document,props.block,path=>{if(path[0]===props.id&&path[1]===name)used=true;return undefined;});
    if(used)setRemoving(name);else removeOutput(name);
  };
  return <div className={styles.panel}>
    <section className={css.group} aria-label={tx('workflows:data.inputs')}>
      <h3 className={css.groupHeading}>{tx('workflows:data.inputs')}<span className={styles.operations}><CodeSignatureButton {...props} onError={setSyncFailed}/><button type="button" className={css.iconButton} aria-label={tx('workflows:data.add-input')} onClick={()=>{let index=1;while(Object.hasOwn(node.variables,`arg${index}`))index++;const name=`arg${index}`;update(current=>({...current,variables:{...current.variables,[name]:{type:'reference',path:[]}}}),at('variables'));}}><IconPlus size={14}/></button></span></h3>
      {syncFailed&&<p role="alert" className={css.hintError}>{tx('workflows:code.sync-error')}</p>}
      <div className={styles.variables}>{Object.entries(node.variables).map(([name,value])=><div className={styles.row} key={name}>
        <div><RouteId label={tx('workflows:data.input-name')} value={name} address={{...at('variables',{key:name}),role:'key'}} rename={(document,from,to)=>{
          if(!/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(to))throw new CommandRejected('invalid_identifier');
          const variables=fieldValue(document,at('variables'))?.value as typeof node.variables;
          if(Object.hasOwn(variables,to))throw new CommandRejected('duplicate_identifier');
          return setField(document,at('variables'),Object.fromEntries(Object.entries(variables).map(([key,item])=>[key===from?to:key,item])));
        }}/></div>
        <div className={styles.reference} data-empty={value.type!=='reference'||!value.path.length||undefined}><ReferencePicker token address={at('variables',{key:name})} path={value.type==='reference'?value.path:[]} edit={props.edit} placeholder={tx('workflows:classifier.select-input')}/></div>
        <button type="button" className={css.iconButton} aria-label={tx('workflows:data.remove-input',{p0:name})} onClick={()=>update(current=>{const {[name]:_,...variables}=current.variables;return {...current,variables};},at('variables'))}><IconTrashOutline16/></button>
      </div>)}</div>
    </section>
    <CodeEditor {...props}/>
    <section className={css.group} aria-label={tx('workflows:prompt.output-variables')}>
      <h3 className={css.groupHeading}><span>{tx('workflows:prompt.output-variables')}<span className={css.required} aria-hidden>*</span></span><button type="button" className={css.iconButton} aria-label={tx('workflows:code.add-output')} onClick={()=>{const name=freshKey(Object.keys(node.outputs),'result');props.edit(document=>{const value=fieldValue(document,at('outputs'))?.value as typeof node.outputs;return setOrder(setField(document,at('outputs'),{...value,[name]:'string'}),orderTarget,[...orderedKeys(document,orderTarget,Object.keys(value)),name]);});}}><IconPlus size={14}/></button></h3>
      <div className={styles.variables}>{outputs.map(name=><div className={clsx(styles.row,styles.output)} key={name}>
        <div><RouteId label={tx('workflows:data.group-name')} value={name} address={{...at('outputs',{key:name}),role:'key'}} rename={(document,from,to)=>renameCodeOutput(document,props.block,props.id,from,to)}/></div>
        <select className={css.input} aria-label={tx('workflows:code.output-type',{p0:name})} value={node.outputs[name]} onChange={event=>{const kind=event.target.value as WorkflowVarType;update(current=>({...current,outputs:{...current.outputs,[name]:kind}}),at('outputs',{key:name}));}}>{TYPES.map(kind=><option key={kind} value={kind}>{tx(`workflows:code.type-${kind}`)}</option>)}</select>
        <button type="button" className={css.iconButton} aria-label={tx('workflows:code.remove-output',{p0:name})} onClick={()=>requestRemove(name)}><IconTrashOutline16/></button>
      </div>)}</div>
      <p className={styles.help}>{tx('workflows:code.output-help')}</p>
    </section>
    <CodeFailure {...props}/>
    {removing!==undefined&&<Modal open title={tx('workflows:code.remove-title')} description={tx('workflows:code.remove-description',{p0:removing})} closeLabel={tx('workflows:dialog.close')} onClose={()=>setRemoving(undefined)} footer={<><Button onClick={()=>setRemoving(undefined)}>{tx('workflows:dialog.cancel')}</Button><Button variant="primary" onClick={()=>{removeOutput(removing);setRemoving(undefined);}}>{tx('workflows:code.confirm-remove')}</Button></>}/>}
  </div>;
}

function CodeEditor(props:PanelProps) {
  const tx=useTranslation(),{node,at}=useNode(props,'code'),address=at('code'),describe=useFieldError();
  const field=useFieldDraft({address,canonical:node.code,command:(document,text)=>({status:'applied',document:setField(document,address,text)})});
  const copy=useCopyFeedback(field.text),expand=useInspectorExpansion();
  const changeLanguage=(language:CodeLanguage)=>{
    if(language===node.code_language)return;
    field.apply();
    props.edit(document=>{
      const owner=blockAt(document,props.block),current=owner&&nodeAt(owner,props.id);if(current?.type!=='code')throw new CommandRejected('missing_node');
      const defaults=codeDefaults(language),removed=new Set(Object.keys(current.outputs).filter(name=>!Object.hasOwn(defaults.outputs,name)));
      const next=replaceNodeExits(document,props.block,props.id,{...current,...defaults,...(current.on_error?.type==='default_value'?{on_error:{type:'default_value' as const,value:{result:''}}}:{})});
      return setOrder(rewriteScopeReferences(next,props.block,path=>path[0]===props.id&&removed.has(path[1])?[]:undefined),{kind:'node_output',address:{block:[...props.block],node:props.id},path:[]},['result']);
    });
  };
  return <div ref={expand.anchor} style={expand.expanded?{height:expand.collapsedHeight}:undefined}>
    <section className={clsx(styles.editor,expand.expanded&&styles.expanded)} style={expand.expanded?expand.bounds:undefined} aria-label={tx('workflows:code.editor')} data-code-expanded={expand.expanded||undefined} onKeyDown={event=>{if(expand.expanded&&event.key==='Escape'&&!event.defaultPrevented){event.preventDefault();event.stopPropagation();expand.setExpanded(false);}}}>
      <header><select aria-label={tx('workflows:code.language')} value={node.code_language} onChange={event=>changeLanguage(event.target.value as CodeLanguage)}><option value="python3">{/* i18n-raw: language name */ 'Python3'}</option><option value="javascript">{/* i18n-raw: language name */ 'JavaScript'}</option></select><div>
        <button type="button" className={css.promptTool} aria-label={tx(copy.copied?'workflows:prompt.copied':'workflows:prompt.copy')} title={tx('workflows:prompt.copy')} onClick={copy.onCopy} disabled={!field.text}>{copy.copied?<IconCheckOutline16/>:<IconCopyOutline16/>}</button>
        <button type="button" className={css.promptTool} aria-label={tx(expand.expanded?'workflows:prompt.collapse':'workflows:prompt.expand')} aria-pressed={expand.expanded} title={tx('workflows:prompt.expand')} onClick={expand.toggle}><IconFullscreenOutline16/></button>
      </div></header>
      <div className={styles.codeArea} data-code-language={node.code_language}><Suspense fallback={<p role="status">{tx('workflows:editor.loading')}</p>}><WorkflowCodeView language={node.code_language} label={tx('workflows:code.editor')} value={field.text} field={encodeField(address)} onChange={field.change} onBlur={field.apply}/></Suspense></div>
      {field.error&&<small role="alert" className={css.hintError}>{describe(field.error)}</small>}<ConflictActions field={field}/>
    </section>
    <p className={styles.help}>{tx('workflows:code.input-help')}</p>
  </div>;
}

function CodeFailure(props:PanelProps) {
  const tx=useTranslation(),{node,at,update}=useNode(props,'code'),strategy=node.on_error??{type:'stop' as const};
  return <>
    <RetrySettings retry={node.retry} setRetry={retry=>update(({retry:_,...current})=>retry?{...current,retry}:current,at('retry'))}/>
    <ErrorStrategy strategy={strategy.type} choose={type=>props.edit(document=>{
      const owner=blockAt(document,props.block),current=owner&&nodeAt(owner,props.id);if(current?.type!=='code')throw new CommandRejected('missing_node');
      const {on_error:_,...rest}=current;
      const value=Object.fromEntries(Object.entries(current.outputs).map(([name,kind])=>[name,kind==='string'?'':kind==='number'?0:kind==='boolean'?false:kind==='object'?{}:[]]));
      const on_error=type==='stop'?undefined:type==='fail_branch'?{type}:{type,value};
      return replaceNodeExits(document,props.block,props.id,on_error?{...rest,on_error}:rest);
    })}>{strategy.type==='default_value'&&<JsonField label={tx('workflows:failure.default-value')} value={strategy.value} address={at('on_error','value')} rows={4}/>}</ErrorStrategy>
  </>;
}
