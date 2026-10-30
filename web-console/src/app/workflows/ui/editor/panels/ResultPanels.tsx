import { useTranslation } from '../../../../../locale/react';
import { blockField } from '../../../document/fields';
import { blockAt } from '../../../document/model';
import { SchemaEditor } from '../forms/SchemaEditor';
import { ValueEditor } from '../forms/ValueEditor';
import { Section } from '../forms/fields';
import { OutputDeclaration, useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

export function ReviewPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'review');
  return <>
    <Section title={tx('workflows:panel.review-subject')}>
      <ValueEditor label={tx('workflows:inspector.review-subject')} address={at('subject', 'value')} value={node.subject.value} edit={props.edit} />
    </Section>
    <Section title={tx('workflows:inspector.review-context')}>
      {node.context.map((item, index) => <div key={index} className={css.nestedRow}>
        <ValueEditor label={`[${index}]`} address={at('context', index)} value={item} edit={props.edit} />
        <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: `[${index}]` })} onClick={() => update(current => ({ ...current, context: current.context.filter((_, at) => at !== index) }), at('context'), { array: at('context'), removed: index })}>×</button>
      </div>)}
      <button type="button" className={css.smallButton} onClick={() => update(current => ({ ...current, context: [...current.context, { type: 'reference', path: ['args'] }] }), at('context'), { array: at('context'), inserted: node.context.length })}>{tx('workflows:inspector.add-item')}</button>
    </Section>
    <OutputDeclaration {...props} />
  </>;
}

export function ReturnPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at } = useNode(props, 'return');
  return <>
    <Section title={tx('workflows:panel.output')}>
      <ValueEditor label={tx('workflows:inspector.return-output')} address={at('output')} value={node.output} edit={props.edit} />
    </Section>
    <SchemaEditor label={tx('workflows:inspector.block-output')} value={blockAt(props.document, props.block)!.output} address={blockField(props.block, ['output'])} edit={props.edit} />
  </>;
}
