import { IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { useTranslation } from '../../../../../locale/react';
import { IconPlus } from '../../icons';
import { ReferencePicker } from '../forms/ReferencePicker';
import { RouteId } from './InferencePanels';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

/** As Dify's End panel: the output variables, each a name and the variable
 * it reads. Together they are the workflow's JSON result. */
export function ReturnPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'return');
  const outputs = (change: (items: typeof node.outputs) => typeof node.outputs) => update(current => ({ ...current, outputs: change(current.outputs) }), at('outputs'));
  return <section className={css.group} aria-label={tx('workflows:end.outputs')}>
    <h3 className={css.groupHeading}>
      <span className={css.headingText}>{tx('workflows:end.outputs')}</span>
      <button type="button" className={css.iconButton} aria-label={tx('workflows:end.add')} title={tx('workflows:end.add')}
        onClick={() => outputs(items => [...items, { variable: '', value_selector: [] }])}><IconPlus size={14} /></button>
    </h3>
    {!node.outputs.length && <p className={css.reviewEmpty}>{tx('workflows:end.empty')}</p>}
    <div className={css.reviewActions}>
      {node.outputs.map((output, index) => <div key={index} className={css.endOutput}>
        <RouteId key={output.variable} value={output.variable} label={tx('workflows:end.name')} className={css.endOutputName}
          onCommit={variable => update(current => ({ ...current, outputs: current.outputs.map((item, i) => i === index ? { ...item, variable } : item) }), at('outputs', index, 'variable'))} />
        <ReferencePicker address={at('outputs', index, 'value_selector')} path={output.value_selector} edit={props.edit} token placeholder={tx('workflows:branch.select-variable')}
          // As Dify: a nameless output takes the variable's own name.
          onChoose={choice => update(current => ({ ...current, outputs: current.outputs.map((item, i) => i === index
            ? { variable: item.variable || choice.path[choice.path.length - 1], value_selector: choice.path } : item) }), at('outputs', index, 'value_selector'))} />
        <button type="button" className={css.iconButton} aria-label={tx('workflows:end.remove', { p0: output.variable || String(index + 1) })}
          onClick={() => outputs(items => items.filter((_, i) => i !== index))}><IconTrashOutline16 /></button>
      </div>)}
    </div>
  </section>;
}
