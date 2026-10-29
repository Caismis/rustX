import { useState } from 'react';
import clsx from 'clsx';
import type { WorkflowReferenceChoice } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { addBranchCase, removeBranchCase } from '../../../document/commands';
import { conditionsOf } from '../../../document/branch';
import { blockAt, nodeAt, type Predicate, type Value } from '../../../document/model';
import { setField, within, type ArrayEdit, type FieldAddress } from '../../../document/fields';
import { IconPlus } from '../../icons';
import { PredicateEditor } from '../forms/ValueEditor';
import { ReferencePicker } from '../forms/ReferencePicker';
import { DraftTextField, SelectField } from '../forms/fields';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

const unfinished = (): Predicate => ({ type: 'equal', left: { type: 'reference', path: [] }, right: { type: 'reference', path: [] } });

export function BranchPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'branch');
  const [picking, setPicking] = useState<string>();
  const [advanced, setAdvanced] = useState(false);
  const change = (caseId: string, condition: Predicate, array?: ArrayEdit) => update(current => ({ ...current, cases: current.cases.map(item => item.id === caseId ? { ...item, condition } : item) }), at('cases', node.cases.findIndex(item => item.id === caseId), 'condition'), array);
  return <section className={css.branchSettings} aria-label={tx('workflows:branch.settings')}>
    <div className={css.branchTabs}><span>{tx('workflows:branch.settings')}</span><button type="button" className={css.branchAdvanced} aria-pressed={advanced} onClick={() => setAdvanced(value => !value)}>{tx('workflows:branch.advanced')}</button></div>
    {node.cases.map((item, index) => {
      const conditions = conditionsOf(item.condition), grouped = item.condition.type === 'and' || item.condition.type === 'or';
      const base = at('cases', index, 'condition');
      return <section key={item.id} className={css.branchCase} aria-label={tx(index ? 'workflows:branch.elif-case' : 'workflows:branch.if', { p0: String(index + 1) })}>
        <header className={css.branchCaseHeader}>
          <div className={css.branchCaseName}><b>{tx(index ? 'workflows:branch.elif' : 'workflows:branch.if')}</b>{node.cases.length > 1 && <small>{tx('workflows:branch.case', { p0: String(index + 1) })}</small>}</div>
          <button type="button" className={css.branchAddCondition} onClick={() => {
            const position = conditions.length;
            change(item.id, { type: item.condition.type === 'or' ? 'or' : 'and', predicates: [...conditions, unfinished()] }, grouped ? { array: within(base, ['predicates']), inserted: position } : undefined);
            setPicking(`${item.id}:${position}`);
          }}><IconPlus size={13} />{tx('workflows:branch.add-condition')}</button>
          {node.cases.length > 1 && <button type="button" className={css.branchRemoveCase} aria-label={tx('workflows:branch.remove-case', { p0: tx('workflows:branch.case', { p0: String(index + 1) }) })}
            onClick={() => props.edit(document => removeBranchCase(document, props.block, props.id, item.id), undefined, { array: at('cases'), removed: index })}>× <span>{tx('workflows:branch.remove')}</span></button>}
        </header>
        {advanced ? <PredicateEditor label={tx('workflows:branch.case', { p0: String(index + 1) })} address={base} predicate={item.condition} edit={props.edit} /> : <div className={css.branchConditionList}>
          {conditions.length > 1 && <SelectField label={tx('workflows:branch.match')} value={item.condition.type === 'or' ? 'or' : 'and'}
            options={[{ value: 'and', label: tx('workflows:branch.and') }, { value: 'or', label: tx('workflows:branch.or') }]}
            onChange={type => change(item.id, { type, predicates: [...conditions] })} />}
          {conditions.map((condition, position) => {
            const address = grouped ? within(base, ['predicates', position]) : base;
            return <div className={css.branchConditionRow} key={position}>
              <ConditionRow {...props} address={address} condition={condition} initiallyOpen={picking === `${item.id}:${position}`} onPicked={() => setPicking(undefined)} />
              <button type="button" className={css.branchRemoveCondition} aria-label={tx('workflows:branch.remove-condition', { p0: String(position + 1) })} onClick={() => {
                const remaining = conditions.filter((_, i) => i !== position);
                props.edit(document => {
                  const owner = blockAt(document, props.block), current = owner && nodeAt(owner, props.id);
                  if (current?.type !== 'branch') return document;
                  return setField(document, base, { type: item.condition.type === 'or' ? 'or' : 'and', predicates: remaining });
                }, undefined, grouped ? { array: within(base, ['predicates']), removed: position } : undefined);
              }}>×</button>
            </div>;
          })}
        </div>}
      </section>;
    })}
    <button type="button" className={css.branchAddCase} disabled={node.cases.length >= 32} onClick={() => props.edit(document => addBranchCase(document, props.block, props.id), undefined, { array: at('cases'), inserted: node.cases.length })}><IconPlus size={14} />{tx('workflows:branch.elif')}</button>
    <section className={css.branchElse}><b>{tx('workflows:branch.else')}</b><p>{tx('workflows:branch.else-description')}</p></section>
  </section>;
}

function ConditionRow({ address, condition, edit, initiallyOpen, onPicked }: PanelProps & {
  address: FieldAddress; condition: Predicate; initiallyOpen: boolean; onPicked: () => void;
}) {
  const tx = useTranslation();
  if (condition.type !== 'boolean' && condition.type !== 'equal' && condition.type !== 'not_equal') return <PredicateEditor label={tx('workflows:branch.condition')} address={address} predicate={condition} edit={edit} />;
  const comparable = (value: Value) => value.type === 'reference' || (value.type === 'literal' && (value.value === null || typeof value.value === 'boolean' || typeof value.value === 'string'));
  const simple = condition.type === 'boolean' ? condition.value.type === 'reference'
    : (condition.type === 'equal' || condition.type === 'not_equal') && condition.left.type === 'reference' && comparable(condition.right);
  if (!simple) return <PredicateEditor label={tx('workflows:branch.condition')} address={address} predicate={condition} edit={edit} />;
  const left = condition.type === 'boolean' ? condition.value : condition.left;
  const right: Value = condition.type === 'boolean' ? { type: 'literal', value: true } : condition.right;
  const operator = condition.type === 'not_equal' ? 'not_equal' : 'equal';
  const replace = (predicate: Predicate) => edit(document => setField(document, address, predicate));
  const picked = (choice: WorkflowReferenceChoice) => {
    const value = choice.schema?.type === 'boolean' ? true : choice.schema?.type === 'null' ? null : '';
    replace({ type: operator, left: { type: 'reference', path: choice.path }, right: { type: 'literal', value } }); onPicked();
  };
  return <div className={css.branchConditionControls}>
    <ReferencePicker address={within(address, [condition.type === 'boolean' ? 'value' : 'left'])} path={left.type === 'reference' ? left.path : []}
      edit={edit} conditionVariable initiallyOpen={initiallyOpen} onChoose={picked} placeholder={tx('workflows:branch.select-variable')} />
    <select className={clsx(css.input, css.branchOperator)} aria-label={tx('workflows:branch.operator')} value={operator}
      onChange={event => replace({ type: event.target.value as 'equal' | 'not_equal', left, right })}>
      <option value="equal">{tx('workflows:branch.is')}</option><option value="not_equal">{tx('workflows:branch.is-not')}</option>
    </select>
    {right.type === 'reference' ? <ReferencePicker address={within(address, ['right'])} path={right.path} edit={edit} />
      : right.type === 'literal' && typeof right.value === 'boolean' ? <select className={css.input} aria-label={tx('workflows:branch.value')} value={String(right.value)} onChange={event => replace({ type: operator, left, right: { type: 'literal', value: event.target.value === 'true' } })}><option value="true">{tx('workflows:port.true')}</option><option value="false">{tx('workflows:port.false')}</option></select>
        : right.type === 'literal' && typeof right.value === 'string' ? <DraftTextField label={tx('workflows:branch.value')} value={right.value} address={within(address, ['right', 'value'])} />
          : <span className={css.branchNull}>null</span>}
  </div>;
}
