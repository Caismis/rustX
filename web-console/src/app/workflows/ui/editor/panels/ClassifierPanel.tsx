import clsx from 'clsx';
import { useId, useLayoutEffect, useRef, useState, type CSSProperties } from 'react';
import { useTranslation } from '../../../../../locale/react';
import { IconChevronDownOutline14, IconCloseOutline16, IconQuestionOutline14 } from '../../../../../presentation/primitives/icons';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { Tooltip } from '../../../../../presentation/primitives/Tooltip';
import { CommandRejected, freshKey, replaceNodeExits } from '../../../document/commands';
import { encodeField, setField, type FieldAddress } from '../../../document/fields';
import { blockAt, nodeAt, type Value, type WorkflowDocument } from '../../../document/model';
import { IconPlus } from '../../icons';
import { ModelSelection } from '../forms/ModelSelection';
import { PromptCard } from '../forms/PromptCard';
import { PromptEditor } from '../forms/PromptEditor';
import { ReferencePicker } from '../forms/ReferencePicker';
import { ConflictActions, useFieldError } from '../forms/fields';
import { useFieldDraft } from '../forms/drafts';
import { promptText } from '../summary';
import { InsertVariable } from './InferencePanels';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';
import styles from './ClassifierPanel.module.css';

type Classifier = Extract<import('../../../document/model').NodeDefinition, {type:'classifier'}>;
type Topic = Classifier['classes'][number];

/** Dify's classifier panel over native input analysis, typed prompts and stable exits. */
export function ClassifierPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'classifier');
  const [classesOpen, setClassesOpen] = useState(true), [advanced, setAdvanced] = useState(true), [outputs, setOutputs] = useState(true);
  const [dragging, setDragging] = useState<string>(), [over, setOver] = useState<string>(), [announcement, setAnnouncement] = useState('');
  const pointer = useRef<{id:string; target?:string} | undefined>(undefined), input=useRef<HTMLDivElement>(null);
  const change = (apply: (classes: Topic[]) => Topic[], array?: import('../../../document/fields').ArrayEdit) => props.edit(document => {
    const owner = blockAt(document, props.block), current = owner && nodeAt(owner, props.id);
    if (current?.type !== 'classifier') throw new CommandRejected('missing_node');
    return replaceNodeExits(document, props.block, props.id, {...current, classes:apply(current.classes)});
  }, undefined, array);
  const move = (id: string, to: number) => {
    const from = node.classes.findIndex(item => item.id === id);
    if (from < 0 || to < 0 || to >= node.classes.length || from === to) return;
    change(classes => { const next = [...classes], index = next.findIndex(item => item.id === id); if (index < 0) throw new CommandRejected('missing_field'); const [item] = next.splice(index,1); next.splice(to,0,item); return next; }, {array:at('classes'),moved:{from,to}});
    setAnnouncement(tx('workflows:classifier.moved',{p0:node.classes[from].label,p1:to+1}));
  };
  return <div className={styles.panel}>
    <ModelSelection policy={node.model ?? {mode:'session'}} address={at('model')} port={props.port} onChange={(model,path) => update(current => ({...current,model}),at('model',...path))}/>
    <section className={css.group}>
      <h3 className={css.groupHeading}>{tx('workflows:classifier.input')}<span className={css.required} aria-hidden>*</span></h3>
      <div ref={input} className={styles.inputVariable} data-empty={node.input.type !== 'reference' || !node.input.path.length || undefined}><ReferencePicker address={at('input')} path={node.input.type === 'reference' ? node.input.path : []} edit={props.edit} token placeholder={tx('workflows:classifier.select-input')}/>
        {node.input.type === 'reference' && node.input.path.length > 0 && <button type="button" className={styles.clearInput} aria-label={tx('workflows:classifier.clear-input')} title={tx('workflows:classifier.clear-input')} onClick={() => {update(current => ({...current,input:{type:'reference',path:[]}}),at('input'));input.current?.querySelector<HTMLButtonElement>('button[aria-haspopup]')?.focus();}}><IconCloseOutline16/></button>}
      </div>
    </section>
    <section className={clsx(css.group, styles.vision)}>
      <h3 className={css.groupHeading}><span>{tx('workflows:classifier.vision')} <Tooltip label={tx('workflows:classifier.vision-unavailable')}><span className={css.promptHint} tabIndex={0}><IconQuestionOutline14/></span></Tooltip></span>
        <Switch label={tx('workflows:classifier.vision')} checked={false} disabled title={tx('workflows:classifier.vision-unavailable')} onChange={() => {}}/>
      </h3>
    </section>
    <section className={css.group} aria-label={tx('workflows:data.classes')}>
      <h3 className={css.groupHeading}><button type="button" className={css.outputToggle} aria-expanded={classesOpen} onClick={() => setClassesOpen(value => !value)}>
        {tx('workflows:data.classes')}<span className={css.required} aria-hidden>*</span><IconChevronDownOutline14/>
      </button></h3>
      <span className={styles.announcement} role="status" aria-live="polite">{announcement}</span>
      {classesOpen && <>
        <p className={styles.renameHint}>{tx('workflows:classifier.rename-hint')}</p>
        <div className={styles.classes}>
          {node.classes.map((item,index) => <div key={item.id} data-classifier-class={item.id} data-drop-target={over === item.id || undefined} className={clsx(styles.item, dragging === item.id && styles.dragging)}>
            <button type="button" className={styles.dragHandle} disabled={node.classes.length < 2} aria-label={tx('workflows:classifier.move',{p0:item.label})} title={tx('workflows:classifier.move-hint')} aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
              onKeyDown={event => { if (event.altKey && ['ArrowUp','ArrowDown'].includes(event.key)) { event.preventDefault(); move(item.id,index + (event.key === 'ArrowUp' ? -1 : 1)); } }}
              onPointerDown={event => { if (event.button !== 0) return; event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); pointer.current = {id:item.id}; setDragging(item.id); }}
              onPointerMove={event => { if (!pointer.current) return; const target = document.elementFromPoint(event.clientX,event.clientY)?.closest<HTMLElement>('[data-classifier-class]')?.dataset.classifierClass; pointer.current.target = target; setOver(target); }}
              onPointerUp={event => { if (!pointer.current) return; const {id,target} = pointer.current; pointer.current = undefined; if (target) move(id,node.classes.findIndex(item => item.id === target)); event.currentTarget.releasePointerCapture(event.pointerId); setDragging(undefined); setOver(undefined); }}
              onPointerCancel={() => { pointer.current = undefined; setDragging(undefined); setOver(undefined); }}><span aria-hidden>⠿</span></button>
            <ClassCard {...props} item={item} index={index} onRemove={() => change(classes => classes.filter(value => value.id !== item.id),{array:at('classes'),removed:index})}/>
          </div>)}
        </div>
        <button type="button" className={css.promptAddMessage} disabled={node.classes.length >= 32} onClick={() => {
          const id = freshKey(node.classes.map(item => item.id),'class');
          change(classes => [...classes,{id,label:tx('workflows:classifier.default-label',{p0:classes.length+1}),description:[]}],{array:at('classes'),inserted:node.classes.length});
        }}><IconPlus size={13}/>{tx('workflows:data.add-class')}</button>
      </>}
    </section>
    <section className={css.group}>
      <h3 className={css.groupHeading}><button type="button" className={css.outputToggle} aria-expanded={advanced} onClick={() => setAdvanced(value => !value)}>{tx('workflows:classifier.advanced')}<IconChevronDownOutline14/></button></h3>
      {advanced && <ClassifierPrompt {...props} parts={node.instruction ?? []} address={at('instruction')} label={tx('workflows:data.instructions')} hint={tx('workflows:classifier.instruction-hint')} onChange={(instruction,session) => update(current => ({...current,instruction}),at('instruction'),undefined,`${encodeField(at('instruction'))}:${session}`)}/> }
    </section>
    <section className={css.group}>
      <h3 className={css.groupHeading}><button type="button" className={css.outputToggle} aria-expanded={outputs} onClick={() => setOutputs(value => !value)}>{tx('workflows:prompt.output-variables')}<IconChevronDownOutline14/></button></h3>
      {outputs && <dl className={css.outputVars}>{(['class_name','class_label','usage'] as const).map(name => <div key={name}><dt><b>{name}</b><code>{/* i18n-raw: native JSON Schema type */ name === 'usage' ? /* i18n-raw: native JSON Schema type */ 'object' : /* i18n-raw: native JSON Schema type */ 'string'}</code></dt><dd>{tx(`workflows:classifier.output-${name}`)}</dd></div>)}</dl>}
    </section>
  </div>;
}

function ClassCard(props: PanelProps & {item:Topic; index:number; onRemove:()=>void}) {
  const tx = useTranslation(), {at,update} = useNode(props,'classifier'), {item,index} = props;
  const address = at('classes',index,'description');
  return <ClassifierPrompt {...props} parts={item.description} address={address} label={tx('workflows:classifier.topic',{p0:item.label})} role={<ClassLabel label={item.label} address={at('classes',index,'label')} defaultLabel={tx('workflows:classifier.default-label',{p0:index+1})}/>}
    onRemove={props.onRemove} removeLabel={tx('workflows:data.remove-class',{p0:item.label})} placeholder={tx('workflows:classifier.topic-placeholder')}
    onChange={(description,session) => update(current => ({...current,classes:current.classes.map(value => value.id === item.id ? {...value,description}:value)}),address,undefined,`${encodeField(address)}:${session}`)}/>;
}

function ClassifierPrompt(props: PanelProps & {parts:readonly Value[]; address:FieldAddress; label:string; role?:import('react').ReactNode; hint?:string; placeholder?:string; onRemove?:()=>void; removeLabel?:string; onChange:(parts:Value[],session:number)=>void}) {
  const tx = useTranslation(), [expanded,setExpanded] = useState(false), [session,setSession] = useState(0);
  const anchor=useRef<HTMLDivElement>(null), [collapsedHeight,setCollapsedHeight]=useState(0), [bounds,setBounds]=useState<CSSProperties>();
  useLayoutEffect(()=>{
    if(!expanded)return;
    const body=anchor.current?.closest<HTMLElement>('[data-workflow-inspector-body]');
    if(!body)return;
    const measure=()=>{
      const box=body.getBoundingClientRect(), viewport=window.visualViewport;
      const bottom=viewport ? viewport.offsetTop+viewport.height : window.innerHeight;
      setBounds({top:box.top,left:box.left,width:box.width,height:Math.max(64,Math.min(box.bottom,bottom)-box.top)});
    };
    measure();const observer=new ResizeObserver(measure);observer.observe(body);
    window.addEventListener('resize',measure);window.visualViewport?.addEventListener('resize',measure);
    return()=>{observer.disconnect();window.removeEventListener('resize',measure);window.visualViewport?.removeEventListener('resize',measure);};
  },[expanded]);
  const toggle=()=>{if(!expanded)setCollapsedHeight(anchor.current?.getBoundingClientRect().height ?? 0);setExpanded(value=>!value);};
  return <PromptEditor parts={props.parts} address={props.address} edit={props.edit} label={props.label} expanded={expanded} placeholder={props.placeholder ?? tx('workflows:classifier.instruction-placeholder')} variablePlaceholder={tx('workflows:branch.select-variable')} onChange={parts => props.onChange(parts,session)}>
    {editable => <div ref={anchor} style={expanded ? {height:collapsedHeight} : undefined}>
      <div data-classifier-expanded={expanded || undefined} className={expanded ? styles.expanded : undefined} style={expanded ? bounds : undefined}
        onKeyDown={event=>{if(expanded && event.key==='Escape' && !event.defaultPrevented){event.preventDefault();event.stopPropagation();setExpanded(false);}}}>
        <PromptCard label={props.label} role={props.role ?? <b className={css.promptRoleFixed}>{props.label}</b>} hint={props.hint} text={promptText(props.parts)} tools={<InsertVariable/>} expanded={expanded} onExpand={toggle} onRemove={props.onRemove} removeLabel={props.removeLabel}>
          <div onFocus={() => setSession(value => value+1)}>{editable}</div>
        </PromptCard>
      </div>
    </div>}
  </PromptEditor>;
}

function ClassLabel({label,address,defaultLabel}:{label:string; address:FieldAddress; defaultLabel:string}) {
  const tx = useTranslation(), [editing,setEditing] = useState(false), input = useRef<HTMLInputElement>(null), id = useId(), describe = useFieldError();
  const command = (document:WorkflowDocument,text:string) => {
    const value = JSON.parse(text) as string;
    return {status:'applied' as const,document:setField(document,address,value.trim() || defaultLabel)};
  };
  const field = useFieldDraft({address,canonical:JSON.stringify(label),command,onApplied:() => setEditing(false)});
  const text = JSON.parse(field.text) as string;
  return <div className={styles.label}>
    {editing ? <input ref={input} data-workflow-field={encodeField(address)} autoFocus id={id} aria-label={tx('workflows:classifier.label')} maxLength={128} value={text} onFocus={event => event.currentTarget.select()}
      onChange={event => field.change(JSON.stringify(event.target.value))} onBlur={field.apply} onKeyDown={event => { if (event.nativeEvent.isComposing) return; if (event.key === 'Enter') {event.preventDefault();field.apply();} else if (event.key === 'Escape') {event.preventDefault();event.stopPropagation();field.reset();setEditing(false);} }}/>
      : <button type="button" title={label} aria-label={tx('workflows:classifier.rename',{p0:label})} onDoubleClick={() => setEditing(true)} onPointerUp={event => {if(event.pointerType === 'touch') setEditing(true);}} onKeyDown={event => {if (event.key === 'Enter' || event.key === ' ') {event.preventDefault();setEditing(true);}}}>{label}</button>}
    {field.error && <small role="alert" className={css.hintError}>{describe(field.error)}</small>}<ConflictActions field={field}/>
  </div>;
}
