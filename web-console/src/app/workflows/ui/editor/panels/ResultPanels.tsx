import { useId } from 'react';
import { useTranslation } from '../../../../../locale/react';
import { blockField, encodeField } from '../../../document/fields';
import { blockAt } from '../../../document/model';
import { SchemaEditor } from '../forms/SchemaEditor';
import { ValueEditor } from '../forms/ValueEditor';
import { NumberField, Section, SelectField, TextField } from '../forms/fields';
import { OutputDeclaration, useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

export function ToolPanel(props: PanelProps) {
  const tx = useTranslation(), listId = useId(), { node, at, update } = useNode(props, 'tool');
  return <>
    <SelectField label={tx('workflows:inspector.tool-origin')} value={node.selector.origin} options={[{ value: 'builtin', label: tx('workflows:inspector.tool-origin-builtin') }, { value: 'source', label: tx('workflows:inspector.tool-origin-source') }]}
      onChange={origin => { if (origin !== node.selector.origin) update(current => ({ ...current, selector: origin === 'builtin' ? { origin, name: current.selector.name } : { origin, source_id: '', name: current.selector.name } }), at('selector')); }} />
    {node.selector.origin === 'source' && <TextField label={tx('workflows:inspector.tool-source')} value={node.selector.source_id} field={encodeField(at('selector', 'source_id'))}
      onChange={(source_id, session) => update(current => ({ ...current, selector: { origin: 'source', source_id, name: current.selector.name } }), at('selector', 'source_id'), undefined, session)} />}
    <TextField label={tx('workflows:inspector.tool-name')} value={node.selector.name} list={node.selector.origin === 'builtin' ? listId : undefined} invalid={!node.selector.name} field={encodeField(at('selector', 'name'))}
      onChange={(name, session) => update(current => ({ ...current, selector: { ...current.selector, name } }), at('selector', 'name'), undefined, session)} />
    <datalist id={listId}>{props.vocabulary?.builtin_tools.map(name => <option key={name} value={name} />)}</datalist>
    <p className={css.note}>{tx('workflows:panel.tool-schema')}</p>
    <ValueEditor label={tx('workflows:inspector.tool-arguments')} address={at('arguments')} value={node.arguments} edit={props.edit} />
    <SelectField label={tx('workflows:inspector.tool-result')} value={node.result.type} options={[{ value: 'json', label: tx('workflows:inspector.tool-result-json') }, { value: 'text', label: tx('workflows:inspector.tool-result-text') }]}
      onChange={type => { if (type !== node.result.type) update(current => ({ ...current, result: type === 'json' ? { type, part: current.result.part, schema: { type: 'object', properties: {}, additionalProperties: false } } : { type, part: current.result.part } }), at('result')); }} />
    <NumberField label={tx('workflows:inspector.tool-part')} value={node.result.part} address={at('result', 'part')} />
    {node.result.type === 'json' && <SchemaEditor label={tx('workflows:inspector.tool-schema')} value={node.result.schema} address={at('result', 'schema')} edit={props.edit} />}
    <OutputDeclaration {...props} />
  </>;
}

export function ReviewPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'review');
  return <>
    <ValueEditor label={tx('workflows:inspector.review-subject')} address={at('subject', 'value')} value={node.subject.value} edit={props.edit} />
    <Section title={tx('workflows:inspector.review-context')}>
      {node.context.map((item, index) => <div key={index} className={css.nestedRow}>
        <ValueEditor label={`[${index}]`} address={at('context', index)} value={item} edit={props.edit} />
        <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: `[${index}]` })} onClick={() => update(current => ({ ...current, context: current.context.filter((_, at) => at !== index) }), at('context'), { array: at('context'), removed: index })}>×</button>
      </div>)}
      <button type="button" className={css.smallButton} onClick={() => update(current => ({ ...current, context: [...current.context, { type: 'reference', path: ['args'] }] }), at('context'), { array: at('context'), inserted: node.context.length })}>{tx('workflows:inspector.add-item')}</button>
    </Section>
    <p className={css.note}>{tx('workflows:inspector.review-note')}</p>
    <OutputDeclaration {...props} />
  </>;
}

export function ReturnPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at } = useNode(props, 'return');
  return <>
    <ValueEditor label={tx('workflows:inspector.return-output')} address={at('output')} value={node.output} edit={props.edit} />
    <p className={css.note}>{tx('workflows:inspector.return-note')}</p>
    <SchemaEditor label={tx('workflows:inspector.block-output')} value={blockAt(props.document, props.block)!.output} address={blockField(props.block, ['output'])} edit={props.edit} />
  </>;
}
