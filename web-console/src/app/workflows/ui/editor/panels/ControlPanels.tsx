import { useTranslation } from '../../../../../locale/react';
import { addBranch, removeBranch, renameBranch } from '../../../document/commands';
import { blockField, nodeField } from '../../../document/fields';
import { orderedKeys } from '../../../document/presentation';
import { AddKey, PredicateEditor, ValueEditor } from '../forms/ValueEditor';
import { KeyField, NumberField, Section } from '../forms/fields';
import { SchemaEditor } from '../forms/SchemaEditor';
import { OutputDeclaration, useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

export function BranchPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at } = useNode(props, 'branch');
  return <><PredicateEditor label={tx('workflows:inspector.branch-condition')} address={at('condition')} predicate={node.condition} edit={props.edit} /><p className={css.note}>{tx('workflows:inspector.branch-ports')}</p></>;
}

export function LoopPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at } = useNode(props, 'loop');
  const body = [...props.block, { node: props.id }];
  return <>
    <p className={css.note}>{tx('workflows:panel.loop-sequence')}</p>
    <ValueEditor label={tx('workflows:inspector.loop-input')} address={at('input')} value={node.input} edit={props.edit} />
    <SchemaEditor label={tx('workflows:panel.body-input')} value={node.body.input} address={blockField(body, ['input'])} edit={props.edit} />
    <SchemaEditor label={tx('workflows:panel.body-output')} value={node.body.output} address={blockField(body, ['output'])} edit={props.edit} />
    <button type="button" className={css.smallButton} onClick={() => props.navigate(body)}>{tx('workflows:inspector.loop-edit')}</button>
    <PredicateEditor label={tx('workflows:inspector.loop-until')} address={at('until')} predicate={node.until} edit={props.edit} />
    <p className={css.note}>{tx('workflows:inspector.loop-until-hint')}</p>
    <ValueEditor label={tx('workflows:inspector.loop-carry')} address={at('carry')} value={node.carry} edit={props.edit} />
    <NumberField label={tx('workflows:inspector.loop-max')} min={1} max={256} value={node.max_iterations} address={at('max_iterations')} />
    <p className={css.note}>{tx('workflows:inspector.loop-ports')}</p>
    <OutputDeclaration {...props} />
  </>;
}

export function ParallelPanel(props: PanelProps) {
  const tx = useTranslation(), { document, block, id, edit } = props, { node, at } = useNode(props, 'parallel');
  const branches = orderedKeys(document, { kind: 'branches', address: { block: [...block], node: id } }, Object.keys(node.branches));
  return <>
    <p className={css.note}>{tx('workflows:inspector.parallel-note')}</p>
    <Section title={tx('workflows:inspector.parallel-branches')} aside={branches.length}>
      {branches.map(key => {
        const branch = node.branches[key], path = [...block, { node: id, branch: key }];
        return <section className={css.branchPanel} key={key}>
          <KeyField value={key} address={nodeField(block, id, ['branches', { key }], 'key')} label={tx('workflows:inspector.parallel-rename', { p0: key })} rename={(current, from, to) => renameBranch(current, block, id, from, to)} />
          <ValueEditor label={tx('workflows:panel.branch-mapping')} address={at('branches', { key }, 'input')} value={branch.input} edit={edit} />
          <SchemaEditor label={tx('workflows:inspector.block-input')} value={branch.block.input} address={blockField(path, ['input'])} edit={edit} />
          <SchemaEditor label={tx('workflows:inspector.block-output')} value={branch.block.output} address={blockField(path, ['output'])} edit={edit} />
          <div className={css.row}><button type="button" className={css.smallButton} onClick={() => props.navigate(path)}>{tx('workflows:inspector.parallel-edit')}</button>
            <button type="button" className={css.smallButton} disabled={branches.length === 1} onClick={() => edit(document => removeBranch(document, block, id, key))}>{tx('workflows:inspector.parallel-remove', { p0: key })}</button></div>
        </section>;
      })}
      <AddKey existing={branches} label={tx('workflows:inspector.parallel-new-key')} action={tx('workflows:inspector.parallel-add')} address={at('branches')} add={(document, key) => addBranch(document, block, id, key)} />
    </Section>
    <OutputDeclaration {...props} />
  </>;
}
