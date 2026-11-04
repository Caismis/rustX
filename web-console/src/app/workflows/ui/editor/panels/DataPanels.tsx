import type { WorkflowVarType } from '../../../../../../../protocol/app-server/v64';
import { useTranslation } from '../../../../../locale/react';
import { IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { CommandRejected, freshKey, renameAggregateOutput } from '../../../document/commands';
import { fieldValue, setField } from '../../../document/fields';
import { IconPlus } from '../../icons';
import { ReferencePicker } from '../forms/ReferencePicker';
import { PathEditor, ValueEditor } from '../forms/ValueEditor';
import { DraftTextField, JsonField, NumberField, Section } from '../forms/fields';
import { RouteId } from './InferencePanels';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

/** Dify's named input bindings and Jinja editor, over native pure rendering. */
export function TemplatePanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'template');
  return <>
    <Section title={tx('workflows:data.inputs')} aside={<button className={css.iconButton} type="button" aria-label={tx('workflows:data.add-input')} disabled={Object.keys(node.variables).length >= 32}
      onClick={() => { const name = freshKey(Object.keys(node.variables), 'arg'); update(current => ({ ...current, variables: { ...current.variables, [name]: { type: 'reference', path: [] } } }), at('variables')); }}><IconPlus size={14} /></button>}>
      {Object.entries(node.variables).map(([name,value]) => <div className={css.dataRow} key={name}>
        <div className={css.dataRowHead}><RouteId label={tx('workflows:data.input-name')} value={name} address={{...at('variables', {key:name}),role:'key'}}
          rename={(document, from, to) => {
            if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(to) || Object.hasOwn(node.variables,to)) throw new CommandRejected('duplicate_identifier');
            const variables=fieldValue(document,at('variables'))?.value as typeof node.variables;
            const { [from]: value, ...rest } = variables;
            return setField(document,at('variables'),{...rest,[to]:value});
          }} />
          <button className={css.iconButton} type="button" aria-label={tx('workflows:data.remove-input', {p0:name})} onClick={() => update(current => { const { [name]: _, ...variables } = current.variables; return {...current,variables}; }, at('variables'))}><IconTrashOutline16 /></button>
        </div>
        <ValueEditor label={tx('workflows:data.input-value')} address={at('variables',{key:name})} value={value} edit={props.edit} />
      </div>)}
    </Section>
    <Section title={tx('workflows:data.template')} help={tx('workflows:data.template-help')}>
      <div className={css.codeEditor}><DraftTextField label={tx('workflows:data.template-code')} value={node.template} address={at('template')} multiline /></div>
    </Section>
    <Outputs items={[[/* i18n-raw: native output identifier */ 'output', /* i18n-raw: native schema type */ 'string']]} />
  </>;
}

const OPERATORS = ['equal','not_equal','contains','not_contains','starts_with','ends_with','greater','greater_equal','less','less_equal'] as const;

/** Dify's operation order: filter, stable sort, then limit. Field paths are
 * literal components, so keys containing dots remain selectable. */
export function ListPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'list');
  const filter = node.filter, sort = node.sort;
  return <>
    <Section title={tx('workflows:data.input')}>
      <ReferencePicker address={at('input')} path={node.input.type === 'reference' ? node.input.path : []} edit={props.edit} token />
    </Section>
    <Section title={tx('workflows:data.filter')} aside={<Switch label={tx('workflows:data.filter')} checked={!!filter} onChange={enabled => update(({filter:_,...current}) => enabled ? {...current,filter:{path:[],operator:'contains',value:''}} : current,at('filter'))} />}>
      {filter && <>
        <p className={css.note}>{tx('workflows:data.field-help')}</p><PathEditor address={at('filter','path')} path={filter.path} edit={props.edit} />
        <label className={css.inlineField}><span>{tx('workflows:data.operator')}</span><select className={css.input} aria-label={tx('workflows:data.operator')} value={filter.operator}
          onChange={event => { const operator=event.target.value as typeof filter.operator; update(current => ({...current,filter:{...filter,operator}}),at('filter','operator')); }}>
          {OPERATORS.map(operator => <option key={operator} value={operator}>{tx(`workflows:data.op-${operator}`)}</option>)}
        </select></label>
        <label className={css.inlineField}><span>{tx('workflows:data.value-type')}</span><select className={css.input} aria-label={tx('workflows:data.value-type')} value={typeof filter.value}
          onChange={event => update(current => ({...current,filter:{...filter,value:event.target.value === 'number' ? 0 : event.target.value === 'boolean' ? false : ''}}),at('filter','value'))}>
          <option value="string">{tx('workflows:fields.type.string')}</option><option value="number">{tx('workflows:fields.type.number')}</option><option value="boolean">{tx('workflows:fields.type.boolean')}</option>
        </select></label>
        {typeof filter.value === 'string' ? <DraftTextField label={tx('workflows:data.compare-value')} value={filter.value} address={at('filter','value')} />
          : <JsonField label={tx('workflows:data.compare-value')} value={filter.value} address={at('filter','value')} line />}
      </>}
    </Section>
    <Section title={tx('workflows:data.sort')} aside={<Switch label={tx('workflows:data.sort')} checked={!!sort} onChange={enabled => update(({sort:_,...current}) => enabled ? {...current,sort:{path:[],order:'asc'}} : current,at('sort'))} />}>
      {sort && <><p className={css.note}>{tx('workflows:data.field-help')}</p><PathEditor address={at('sort','path')} path={sort.path} edit={props.edit} />
        <label className={css.inlineField}><span>{tx('workflows:data.order')}</span><select className={css.input} aria-label={tx('workflows:data.order')} value={sort.order} onChange={event => update(current => ({...current,sort:{...sort,order:event.target.value as 'asc'|'desc'}}),at('sort','order'))}>
          <option value="asc">{tx('workflows:data.asc')}</option><option value="desc">{tx('workflows:data.desc')}</option>
        </select></label></>}
    </Section>
    <Section title={tx('workflows:data.limit')} aside={<Switch label={tx('workflows:data.limit')} checked={node.limit != null} onChange={enabled => update(({limit:_,...current}) => enabled ? {...current,limit:10} : current,at('limit'))} />}>
      {node.limit != null && <NumberField label={tx('workflows:data.take')} value={node.limit} address={at('limit')} min={1} max={65536} />}
    </Section>
    <Outputs items={[[/* i18n-raw: native output identifier */ 'result', /* i18n-raw: native schema type */ 'array'],['first_record','item?'],['last_record','item?']]} />
  </>;
}

const TYPES: readonly WorkflowVarType[] = ['string','number','boolean','object','array[string]','array[number]','array[boolean]','array[object]'];

export function AggregatePanel(props: PanelProps) {
  const tx=useTranslation(), {node,at,update}=useNode(props,'aggregate');
  return <Section title={tx('workflows:data.groups')} help={tx('workflows:data.aggregate-help')} aside={<button className={css.iconButton} type="button" aria-label={tx('workflows:data.add-group')} disabled={node.groups.length>=32} onClick={() => {
    const name=freshKey(node.groups.map(group=>group.name),'output');
    update(current => ({...current,groups:[...current.groups,{name,output_type:'string',variables:[[]]}]}),at('groups'),{array:at('groups'),inserted:node.groups.length});
  }}><IconPlus size={14} /></button>}>
    {node.groups.map((group,index) => <div className={css.dataRow} key={index}>
      <div className={css.dataRowHead}><RouteId label={tx('workflows:data.group-name')} value={group.name} address={at('groups',index,'name')} rename={(document,from,to) => renameAggregateOutput(document,props.block,props.id,from,to)} />
        <button className={css.iconButton} type="button" aria-label={tx('workflows:data.remove-group',{p0:group.name})} disabled={node.groups.length<2} onClick={() => update(current => ({...current,groups:current.groups.filter((_,i)=>i!==index)}),at('groups'),{array:at('groups'),removed:index})}><IconTrashOutline16 /></button></div>
      <label className={css.inlineField}><span>{tx('workflows:data.output-type')}</span><select className={css.input} aria-label={tx('workflows:data.output-type')} value={group.output_type} onChange={event => update(current => ({...current,groups:current.groups.map((item,i) => i===index ? {...item,output_type:event.target.value as WorkflowVarType}:item)}),at('groups',index,'output_type'))}>
        {TYPES.map(type => <option key={type} value={type}>{type}</option>)}
      </select></label>
      {group.variables.map((path,position) => <div className={css.dataRowHead} key={position}>
        <ReferencePicker address={at('groups',index,'variables',position)} path={path} edit={props.edit} token />
        <button className={css.iconButton} type="button" aria-label={tx('workflows:data.remove-source',{p0:String(position+1)})} disabled={group.variables.length<2} onClick={() => update(current => ({...current,groups:current.groups.map((item,i) => i===index ? {...item,variables:item.variables.filter((_,j)=>j!==position)}:item)}),at('groups',index,'variables'),{array:at('groups',index,'variables'),removed:position})}><IconTrashOutline16 /></button>
      </div>)}
      <button className={css.smallButton} type="button" disabled={group.variables.length>=32} onClick={() => update(current => ({...current,groups:current.groups.map((item,i)=>i===index ? {...item,variables:[...item.variables,[]]}:item)}),at('groups',index,'variables'),{array:at('groups',index,'variables'),inserted:group.variables.length})}><IconPlus size={13} />{tx('workflows:data.add-source')}</button>
      <Outputs items={[[group.name,group.output_type]]} />
    </div>)}
  </Section>;
}

function Outputs({items}:{items:readonly (readonly [string,string])[]}) {
  const tx=useTranslation();
  return <Section title={tx('workflows:prompt.output-variables')}><dl className={css.outputVars}>{items.map(([name,type]) => <div key={name}><dt><b>{name}</b><code>{type}</code></dt></div>)}</dl></Section>;
}
