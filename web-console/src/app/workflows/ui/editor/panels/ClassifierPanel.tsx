import clsx from 'clsx';
import { useId, useRef, useState } from 'react';
import { useTranslation } from '../../../../../locale/react';
import { IconChevronDownOutline14, IconCloseOutline16 } from '../../../../../presentation/primitives/icons';
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
import { editorCss } from '../editorCss';
import { Input as TextInput } from '@/presentation/primitives/Input';

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
  return <div className="min-w-0 [&_h3]:text-[13px] [&_h3]:font-medium [&_h3]:justify-start [&_h3]:gap-[4px] [&>section]:mb-[18px] [&>section:nth-child(n+3)]:pt-[16px] [&>section:nth-child(n+3)]:[border-top:0.5px_solid_var(--dsw-alias-border-l3)]">
    <ModelSelection policy={node.model ?? {mode:'session'}} address={at('model')} port={props.port} onChange={(model,path) => update(current => ({...current,model}),at('model',...path))}/>
    <section className={editorCss.group}>
      <h3 className={editorCss.groupHeading}>{tx('workflows:classifier.input')}<span className={editorCss.required} aria-hidden>*</span></h3>
      <div ref={input} className="group/iv relative [&>div>button]:w-full [&>div>button]:min-h-[32px] [&>div>button]:pt-[4px] [&>div>button]:pr-[32px] [&>div>button]:pb-[4px] [&>div>button]:pl-[10px] [&>div>button]:border-0 [&>div>button]:rounded-[var(--dsw-radius-sm)] [&>div>button]:bg-[var(--dsw-alias-interactive-bg-hover-solid)] [&>div>button]:hover:bg-[var(--dsw-alias-interactive-bg-hover)] [&[data-empty]>div>button]:flex [&[data-empty]>div>button]:items-center [&[data-empty]>div>button]:gap-[6px] [&[data-empty]>div>button]:[color:var(--dsw-alias-label-tertiary)] [&[data-empty]>div>button]:before:content-['{x}'] [&[data-empty]>div>button]:before:[font-family:var(--ds-font-family-code),monospace] [&[data-empty]>div>button]:before:text-[12px]" data-empty={node.input.type !== 'reference' || !node.input.path.length || undefined}><ReferencePicker address={at('input')} path={node.input.type === 'reference' ? node.input.path : []} edit={props.edit} token placeholder={tx('workflows:classifier.select-input')}/>
        {node.input.type === 'reference' && node.input.path.length > 0 && <button type="button" className="absolute right-[4px] top-[4px] inline-flex items-center justify-center w-[24px] h-[24px] border-0 rounded-[var(--dsw-radius-xs)] bg-transparent [color:var(--dsw-alias-label-tertiary)] cursor-pointer opacity-0 group-focus-within/iv:opacity-100 [@media(hover:hover)]:group-hover/iv:opacity-100 [@media(hover:none)]:opacity-100 hover:[color:var(--dsw-alias-label-primary)] hover:bg-[var(--dsw-alias-interactive-bg-hover)]" aria-label={tx('workflows:classifier.clear-input')} title={tx('workflows:classifier.clear-input')} onClick={() => {update(current => ({...current,input:{type:'reference',path:[]}}),at('input'));input.current?.querySelector<HTMLButtonElement>('button[aria-haspopup]')?.focus();}}><IconCloseOutline16/></button>}
      </div>
    </section>
    <section className={editorCss.group} aria-label={tx('workflows:data.classes')}>
      <h3 className={editorCss.groupHeading}><button type="button" className={editorCss.outputToggle} aria-expanded={classesOpen} onClick={() => setClassesOpen(value => !value)}>
        {tx('workflows:data.classes')}<span className={editorCss.required} aria-hidden>*</span><IconChevronDownOutline14/>
      </button></h3>
      <span className="absolute w-[1px] h-[1px] p-0 overflow-hidden [clip-path:inset(50%)] whitespace-nowrap" role="status" aria-live="polite">{announcement}</span>
      {classesOpen && <>
        <p className="mt-0 mx-0 mb-[8px] px-[12px] py-[8px] [border:0.5px_solid_var(--dsw-alias-border-l3)] rounded-[var(--dsw-radius-sm)] [color:var(--dsw-alias-label-tertiary)] text-[12px] leading-[16px]">{tx('workflows:classifier.rename-hint')}</p>
        <div className="grid gap-[10px] mb-[8px]">
          {node.classes.map((item,index) => <div key={item.id} data-classifier-class={item.id} data-drop-target={over === item.id || undefined} className={clsx('group/ci relative min-w-0 rounded-[var(--dsw-radius-md)] data-[drop-target]:[outline:2px_solid_var(--dsw-alias-state-business-primary)] [&:focus-within_header]:pl-[22px] [@media(hover:hover)]:[&:has(.drag-handle:enabled):hover_header]:pl-[22px] [@media(hover:none)]:[&:has(.drag-handle:enabled)_header]:pl-[22px]', dragging === item.id && 'opacity-60')}>
            <button type="button" className="drag-handle absolute top-[4px] left-0 z-[2] w-[20px] h-[24px] border-0 p-0 rounded-[var(--dsw-radius-xs)] bg-transparent [color:var(--dsw-alias-label-tertiary)] opacity-0 cursor-grab touch-none text-[18px] group-focus-within/ci:opacity-100 focus-visible:opacity-100 [@media(hover:hover)]:group-hover/ci:opacity-100 [@media(hover:none)]:opacity-100" disabled={node.classes.length < 2} aria-label={tx('workflows:classifier.move',{p0:item.label})} title={tx('workflows:classifier.move-hint')} aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown"
              onKeyDown={event => { if (event.altKey && ['ArrowUp','ArrowDown'].includes(event.key)) { event.preventDefault(); move(item.id,index + (event.key === 'ArrowUp' ? -1 : 1)); } }}
              onPointerDown={event => { if (event.button !== 0) return; event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId); pointer.current = {id:item.id}; setDragging(item.id); }}
              onPointerMove={event => { if (!pointer.current) return; const target = document.elementFromPoint(event.clientX,event.clientY)?.closest<HTMLElement>('[data-classifier-class]')?.dataset.classifierClass; pointer.current.target = target; setOver(target); }}
              onPointerUp={event => { if (!pointer.current) return; const {id,target} = pointer.current; pointer.current = undefined; if (target) move(id,node.classes.findIndex(item => item.id === target)); event.currentTarget.releasePointerCapture(event.pointerId); setDragging(undefined); setOver(undefined); }}
              onPointerCancel={() => { pointer.current = undefined; setDragging(undefined); setOver(undefined); }}><span aria-hidden>⠿</span></button>
            <ClassCard {...props} item={item} index={index} onRemove={() => change(classes => classes.filter(value => value.id !== item.id),{array:at('classes'),removed:index})}/>
          </div>)}
        </div>
        <button type="button" className={editorCss.promptAddMessage} disabled={node.classes.length >= 32} onClick={() => {
          const id = freshKey(node.classes.map(item => item.id),'class');
          change(classes => [...classes,{id,label:tx('workflows:classifier.default-label',{p0:classes.length+1}),description:[]}],{array:at('classes'),inserted:node.classes.length});
        }}><IconPlus size={13}/>{tx('workflows:data.add-class')}</button>
      </>}
    </section>
    <section className={editorCss.group}>
      <h3 className={editorCss.groupHeading}><button type="button" className={editorCss.outputToggle} aria-expanded={advanced} onClick={() => setAdvanced(value => !value)}>{tx('workflows:classifier.advanced')}<IconChevronDownOutline14/></button></h3>
      {advanced && <ClassifierPrompt {...props} parts={node.instruction ?? []} address={at('instruction')} label={tx('workflows:data.instructions')} hint={tx('workflows:classifier.instruction-hint')} onChange={(instruction,session) => update(current => ({...current,instruction}),at('instruction'),undefined,`${encodeField(at('instruction'))}:${session}`)}/> }
    </section>
    <section className={editorCss.group}>
      <h3 className={editorCss.groupHeading}><button type="button" className={editorCss.outputToggle} aria-expanded={outputs} onClick={() => setOutputs(value => !value)}>{tx('workflows:prompt.output-variables')}<IconChevronDownOutline14/></button></h3>
      {outputs && <dl className={editorCss.outputVars}>{(['class_name','class_label','usage'] as const).map(name => <div key={name}><dt><b>{name}</b><code>{/* i18n-raw: native JSON Schema type */ name === 'usage' ? /* i18n-raw: native JSON Schema type */ 'object' : /* i18n-raw: native JSON Schema type */ 'string'}</code></dt><dd>{tx(`workflows:classifier.output-${name}`)}</dd></div>)}</dl>}
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
  return <PromptEditor parts={props.parts} address={props.address} edit={props.edit} label={props.label} expanded={expanded} placeholder={props.placeholder ?? tx('workflows:classifier.instruction-placeholder')} variablePlaceholder={tx('workflows:branch.select-variable')} onChange={parts => props.onChange(parts,session)}>
    {editable => <PromptCard label={props.label} role={props.role ?? <b className={editorCss.promptRoleFixed}>{props.label}</b>} hint={props.hint} text={promptText(props.parts)} tools={<InsertVariable/>} expanded={expanded} onExpand={()=>setExpanded(value=>!value)} onRemove={props.onRemove} removeLabel={props.removeLabel}>
      <div onFocus={() => setSession(value => value+1)}>{editable}</div>
    </PromptCard>}
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
  return <div className="min-w-0 flex-1 [color:var(--dsw-alias-label-secondary)] text-[12px] font-medium [&>button]:block [&>button]:max-w-full [&>button]:px-0 [&>button]:py-[2px] [&>button]:border-0 [&>button]:rounded-[var(--dsw-radius-xs)] [&>button]:bg-transparent [&>button]:[color:inherit] [&>button]:[font:inherit] [&>button]:text-left [&>button]:text-ellipsis [&>button]:whitespace-nowrap [&>button]:overflow-hidden [&>button]:cursor-text [&>button]:touch-manipulation [&>input]:box-border [&>input]:w-full [&>input]:h-[24px] [&>input]:px-[4px] [&>input]:py-0 [&>input]:[border:1px_solid_var(--dsw-alias-state-business-primary)] [&>input]:rounded-[var(--dsw-radius-xs)] [&>input]:bg-[var(--dsw-alias-bg-layer-1)] [&>input]:[color:inherit] [&>input]:[font:inherit] [&>input]:outline-0">
    {editing ? <TextInput ref={input} data-workflow-field={encodeField(address)} autoFocus id={id} aria-label={tx('workflows:classifier.label')} maxLength={128} value={text} onFocus={event => event.currentTarget.select()}
      onChange={event => field.change(JSON.stringify(event.target.value))} onBlur={field.apply} onKeyDown={event => { if (event.nativeEvent.isComposing) return; if (event.key === 'Enter') {event.preventDefault();field.apply();} else if (event.key === 'Escape') {event.preventDefault();event.stopPropagation();field.reset();setEditing(false);} }}/>
      : <button type="button" title={label} aria-label={tx('workflows:classifier.rename',{p0:label})} onDoubleClick={() => setEditing(true)} onPointerUp={event => {if(event.pointerType === 'touch') setEditing(true);}} onKeyDown={event => {if (event.key === 'Enter' || event.key === ' ') {event.preventDefault();setEditing(true);}}}>{label}</button>}
    {field.error && <small role="alert" className={editorCss.hintError}>{describe(field.error)}</small>}<ConflictActions field={field}/>
  </div>;
}
