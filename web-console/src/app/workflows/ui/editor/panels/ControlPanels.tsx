import { useTranslation } from '../../../../../locale/react';
import { blockField } from '../../../document/fields';
import { PredicateEditor, ValueEditor } from '../forms/ValueEditor';
import { NumberField, Section } from '../forms/fields';
import { SchemaEditor } from '../forms/SchemaEditor';
import { OutputDeclaration, useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

export function LoopPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at } = useNode(props, 'loop');
  const body = [...props.block, { node: props.id }];
  return <>
    <Section title={tx('workflows:panel.loop-input')}>
      <ValueEditor label={tx('workflows:inspector.loop-input')} address={at('input')} value={node.input} edit={props.edit} />
      <SchemaEditor label={tx('workflows:panel.body-input')} value={node.body.input} address={blockField(body, ['input'])} edit={props.edit} />
      <SchemaEditor label={tx('workflows:panel.body-output')} value={node.body.output} address={blockField(body, ['output'])} edit={props.edit} />
      <button type="button" className={css.smallButton} onClick={() => props.navigate(body)}>{tx('workflows:inspector.loop-edit')}</button>
    </Section>
    <Section title={tx('workflows:panel.loop-condition')}>
      <PredicateEditor label={tx('workflows:inspector.loop-until')} address={at('until')} predicate={node.until} edit={props.edit} />
      <p className={css.note}>{tx('workflows:inspector.loop-until-hint')}</p>
    </Section>
    <Section title={tx('workflows:panel.loop-variables')}>
      <ValueEditor label={tx('workflows:inspector.loop-carry')} address={at('carry')} value={node.carry} edit={props.edit} />
      <NumberField label={tx('workflows:inspector.loop-max')} min={1} max={256} value={node.max_iterations} address={at('max_iterations')} />
    </Section>
    <OutputDeclaration {...props} />
  </>;
}
