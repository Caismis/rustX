import { useState } from 'react';
import { useTranslation } from '../../../../../locale/react';
import { IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { addBranchCase, removeBranchCase } from '../../../document/commands';
import type { Predicate } from '../../../document/model';
import type { ArrayEdit } from '../../../document/fields';
import { IconPlus } from '../../icons';
import { PredicateEditor } from '../forms/ValueEditor';
import { ConditionList } from './Conditions';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

export function BranchPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'branch');
  const [advanced, setAdvanced] = useState(false);
  const change = (caseId: string, condition: Predicate, array?: ArrayEdit) => update(current => ({ ...current, cases: current.cases.map(item => item.id === caseId ? { ...item, condition } : item) }), at('cases', node.cases.findIndex(item => item.id === caseId), 'condition'), array);
  return <section className={css.branchSettings} aria-label={tx('workflows:branch.settings')}>
    <div className={css.branchTabs}><span>{tx('workflows:branch.settings')}</span><button type="button" className={css.branchAdvanced} aria-pressed={advanced} onClick={() => setAdvanced(value => !value)}>{tx('workflows:branch.advanced')}</button></div>
    {node.cases.map((item, index) => {
      const base = at('cases', index, 'condition');
      return <section key={item.id} className={css.branchCase} aria-label={tx(index ? 'workflows:branch.elif-case' : 'workflows:branch.if', { p0: String(index + 1) })}>
        <div className={css.branchCaseName}><b>{tx(index ? 'workflows:branch.elif' : 'workflows:branch.if')}</b>{node.cases.length > 1 && <small>{tx('workflows:branch.case', { p0: String(index + 1) })}</small>}</div>
        <div className={css.branchCaseContent}>
          {advanced ? <PredicateEditor label={tx('workflows:branch.case', { p0: String(index + 1) })} address={base} predicate={item.condition} edit={props.edit} />
            : <ConditionList {...props} base={base} condition={item.condition} set={(condition, array) => change(item.id, condition ?? { type: 'and', predicates: [] }, array)} />}
        </div>
        {node.cases.length > 1 && <button type="button" className={css.branchRemoveCase} aria-label={tx('workflows:branch.remove-case', { p0: tx('workflows:branch.case', { p0: String(index + 1) }) })}
          onClick={() => props.edit(document => removeBranchCase(document, props.block, props.id, item.id), undefined, { array: at('cases'), removed: index })}><IconTrashOutline16 /></button>}
      </section>;
    })}
    <button type="button" className={css.branchAddCase} disabled={node.cases.length >= 32} onClick={() => props.edit(document => addBranchCase(document, props.block, props.id), undefined, { array: at('cases'), inserted: node.cases.length })}><IconPlus size={14} />{tx('workflows:branch.elif')}</button>
    <section className={css.branchElse}><b>{tx('workflows:branch.else')}</b><p>{tx('workflows:branch.else-description')}</p></section>
  </section>;
}

