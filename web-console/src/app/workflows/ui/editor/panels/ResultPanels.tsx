import { IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { useTranslation } from '../../../../../locale/react';
import { IconPlus } from '../../icons';
import { ReferencePicker } from '../forms/ReferencePicker';
import { RouteId } from './InferencePanels';
import { freshKey } from '../../../document/commands';
import { setField, type ArrayEdit } from '../../../document/fields';
import { useNode, type PanelProps } from './shared';
import { editorCss } from '../editorCss';

/** As Dify's End panel: the output variables, each a name and the variable
 * it reads. Together they are the workflow's JSON result. */
export function ReturnPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'return');
  const outputs = (change: (items: typeof node.outputs) => typeof node.outputs, array: ArrayEdit) => update(current => ({ ...current, outputs: change(current.outputs) }), at('outputs'), array);
  return <section className={editorCss.group} aria-label={tx('workflows:end.outputs')}>
    <h3 className={editorCss.groupHeading}>
      <span className={editorCss.headingText}>{tx('workflows:end.outputs')}</span>
      <button type="button" className={editorCss.iconButton} aria-label={tx('workflows:end.add')} title={tx('workflows:end.add')}
        onClick={() => outputs(items => [...items, { variable: '', value_selector: [] }], { array: at('outputs'), inserted: node.outputs.length })}><IconPlus size={14} /></button>
    </h3>
    {!node.outputs.length && <p className={editorCss.reviewEmpty}>{tx('workflows:end.empty')}</p>}
    <div className={editorCss.reviewActions}>
      {node.outputs.map((output, index) => <div key={index} className={editorCss.endOutput}>
        <RouteId value={output.variable} label={tx('workflows:end.name')} className={editorCss.endOutputName} address={at('outputs', index, 'variable')} rename={(document, _from, to) => setField(document, at('outputs', index, 'variable'), to)} />
        <ReferencePicker address={at('outputs', index, 'value_selector')} path={output.value_selector} edit={props.edit} token placeholder={tx('workflows:branch.select-variable')}
          // As Dify: a nameless output takes the variable's own name.
          onChoose={choice => update(current => ({ ...current, outputs: current.outputs.map((item, i) => i === index
            ? { variable: item.variable || freshKey(current.outputs.filter((_,position)=>position!==index).map(output=>output.variable),choice.path[choice.path.length - 1]), value_selector: choice.path } : item) }), at('outputs', index, 'value_selector'))} />
        <button type="button" className={editorCss.iconButton} aria-label={tx('workflows:end.remove', { p0: output.variable || String(index + 1) })}
          onClick={() => outputs(items => items.filter((_, i) => i !== index), { array: at('outputs'), removed: index })}><IconTrashOutline16 /></button>
      </div>)}
    </div>
  </section>;
}
