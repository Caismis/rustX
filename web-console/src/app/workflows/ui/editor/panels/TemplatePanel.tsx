import { lazy, Suspense, useState } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
import { IconCheckOutline16, IconChevronDownOutline14, IconCopyOutline16, IconFullscreenOutline16, IconQuestionOutline14, IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { useCopyFeedback } from '../../../../../presentation/primitives/use-copy-feedback';
import { CommandRejected } from '../../../document/commands';
import { encodeField, setField } from '../../../document/fields';
import { blockAt, nodeAt } from '../../../document/model';
import { renameTemplateInput, templateInputName } from '../../../document/template';
import { IconPlus } from '../../icons';
import { ReferencePicker } from '../forms/ReferencePicker';
import { ConflictActions, useFieldError } from '../forms/fields';
import { useFieldDraft } from '../forms/drafts';
import { useInspectorExpansion } from '../forms/useInspectorExpansion';
import type { VariableInsertion } from '../forms/WorkflowCodeView';
import { RouteId } from './InferencePanels';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';
import styles from '../forms/WorkflowCode.module.css';
import templateStyles from './TemplatePanel.module.css';

const WorkflowCodeView=lazy(()=>import('../forms/WorkflowCodeView'));

/** Dify's Template Transform: reference bindings, Jinja source and fixed output. */
export function TemplatePanel(props:PanelProps) {
  const tx=useTranslation(),{node,at,update}=useNode(props,'template'),[outputs,setOutputs]=useState(false);
  return <div className={styles.panel}>
    <section className={css.group} aria-label={tx('workflows:data.inputs')}>
      <h3 className={css.groupHeading}>{tx('workflows:data.inputs')}<button type="button" className={css.iconButton} aria-label={tx('workflows:data.add-input')} disabled={Object.keys(node.variables).length>=32} onClick={()=>{
        update(current=>({...current,variables:{...current.variables,[templateInputName(Object.keys(current.variables))]:{type:'reference',path:[]}}}),at('variables'));
      }}><IconPlus size={14}/></button></h3>
      <div className={styles.variables}>{Object.entries(node.variables).map(([name,value])=><div className={styles.row} key={name}>
        <div><RouteId label={tx('workflows:data.input-name')} value={name} address={{...at('variables',{key:name}),role:'key'}} rename={(document,from,to)=>renameTemplateInput(document,at('variables'),from,to)}/></div>
        <div className={styles.reference} data-empty={value.type!=='reference'||!value.path.length||undefined}><ReferencePicker token address={at('variables',{key:name})} path={value.type==='reference'?value.path:[]} edit={props.edit} placeholder={tx('workflows:classifier.select-input')}/></div>
        <button type="button" className={css.iconButton} aria-label={tx('workflows:data.remove-input',{p0:name})} onClick={()=>update(current=>{const {[name]:_,...variables}=current.variables;return {...current,variables};},at('variables'))}><IconTrashOutline16/></button>
      </div>)}</div>
    </section>
    <TemplateEditor key={encodeField(at('template'))} {...props}/>
    <section className={css.group}>
      <h3 className={css.groupHeading}><button type="button" className={css.outputToggle} aria-expanded={outputs} onClick={()=>setOutputs(value=>!value)}>{tx('workflows:prompt.output-variables')}<IconChevronDownOutline14/></button></h3>
      {outputs&&<dl className={css.outputVars}><div><dt><b>{/* i18n-raw: native output identifier */ 'output'}</b><code>{/* i18n-raw: native schema type */ 'string'}</code></dt><dd>{tx('workflows:template.output-description')}</dd></div></dl>}
    </section>
  </div>;
}

function TemplateEditor(props:PanelProps) {
  const tx=useTranslation(),{node,at}=useNode(props,'template'),address=at('template'),describe=useFieldError();
  const field=useFieldDraft({address,canonical:node.template,command:(document,text)=>({status:'applied',document:setField(document,address,text)})});
  const copy=useCopyFeedback(field.text),expand=useInspectorExpansion(),[insertion,setInsertion]=useState<VariableInsertion>(),[limit,setLimit]=useState(false);
  return <div ref={expand.anchor} style={expand.expanded?{height:expand.collapsedHeight}:undefined}>
    <section className={clsx(styles.editor,expand.expanded&&styles.expanded)} style={expand.expanded?expand.bounds:undefined} aria-label={tx('workflows:data.template-code')} data-template-expanded={expand.expanded||undefined} onKeyDown={event=>{
      if(event.key==='Escape'&&!event.defaultPrevented&&expand.expanded){event.preventDefault();event.stopPropagation();expand.setExpanded(false);}
    }}>
      <header><span>{tx('workflows:template.code')}</span><div className={templateStyles.tools}>
        <a href="https://jinja.palletsprojects.com/en/3.1.x/templates/" target="_blank" rel="noreferrer" className={templateStyles.jinja}>{tx('workflows:template.jinja-only')}<IconQuestionOutline14/></a>
        <button type="button" className={css.promptTool} aria-label={tx(copy.copied?'workflows:prompt.copied':'workflows:prompt.copy')} title={tx('workflows:prompt.copy')} onClick={copy.onCopy} disabled={!field.text}>{copy.copied?<IconCheckOutline16/>:<IconCopyOutline16/>}</button>
        <button type="button" className={css.promptTool} aria-label={tx(expand.expanded?'workflows:prompt.collapse':'workflows:prompt.expand')} aria-pressed={expand.expanded} onClick={expand.toggle}><IconFullscreenOutline16/></button>
      </div></header>
      <div className={styles.codeArea}><Suspense fallback={<p role="status">{tx('workflows:editor.loading')}</p>}><WorkflowCodeView language="jinja2" label={tx('workflows:data.template-code')} value={field.text} field={encodeField(address)} onChange={field.change} onBlur={field.apply} onVariableRequest={request=>{setLimit(false);setInsertion(request);}}/></Suspense></div>
      {insertion&&<ReferencePicker key={`${insertion.from}:${insertion.source}`} token initiallyOpen anchor={insertion.rect} address={at('variables',{key:templateInputName(Object.keys(node.variables))})} path={[]} edit={props.edit} onClose={restore=>{setInsertion(undefined);if(restore)insertion.focus();}} onChoose={choice=>{
        if(!insertion.current())return;
        const existing=Object.entries(node.variables).find(([,value])=>value.type==='reference'&&JSON.stringify(value.path)===JSON.stringify(choice.path));
        if(!existing&&Object.keys(node.variables).length>=32){setLimit(true);return;}
        const name=existing?.[0]??templateInputName(Object.keys(node.variables),choice.path.at(-1));
        field.apply();
        if(!insertion.complete(name))return;
        // Source plus a new binding form one semantic edit. The CodeMirror
        // transaction above owns the caret; the editor owns the document.
        field.reset();
        props.edit(document=>{
          const owner=blockAt(document,props.block),current=owner&&nodeAt(owner,props.id);
          if(current?.type!=='template')throw new CommandRejected('missing_node');
          if(current.template!==insertion.source)throw new CommandRejected('pending_input');
          const source=insertion.source.slice(0,insertion.from)+`{{ ${name} }}`+insertion.source.slice(insertion.to);
          const next=setField(document,address,source);
          return existing?next:setField(next,at('variables'),{...current.variables,[name]:{type:'reference',path:[...choice.path]}});
        });
      }}/>}
      {field.error&&<small role="alert" className={css.hintError}>{describe(field.error)}</small>}<ConflictActions field={field}/>
      {limit&&<small role="alert" className={css.hintError}>{tx('workflows:template.input-limit')}</small>}
    </section>
    <p className={styles.help}>{tx('workflows:template.insert-help')}</p>
  </div>;
}
