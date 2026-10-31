import { useState } from 'react';
import { Menu } from '@base-ui/react/menu';
import clsx from 'clsx';
import type { WorkflowReferenceChoice } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { IconChevronDownOutline14, IconRefreshOutline14, IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { conditionsOf } from '../../../document/branch';
import type { Predicate, Value } from '../../../document/model';
import { setField, within, type ArrayEdit, type FieldAddress } from '../../../document/fields';
import { IconPlus } from '../../icons';
import { PredicateEditor } from '../forms/ValueEditor';
import { ReferencePicker } from '../forms/ReferencePicker';
import { DraftTextField } from '../forms/fields';
import type { PanelProps } from './shared';
import css from '../Editor.module.css';

const unfinished = (): Predicate => ({ type: 'equal', left: { type: 'reference', path: [] }, right: { type: 'reference', path: [] } });
const stringOperators = ['contains', 'not_contains', 'starts_with', 'ends_with', 'equal', 'not_equal', 'empty', 'not_empty'] as const;
type SimpleOperator = typeof stringOperators[number];

/** As Dify's ConditionList, shared by IF/ELSE cases and a Loop's break
 * conditions: conditions joined by a bracket whose button toggles AND/OR,
 * each a variable, an operator and a value. `set` receives `undefined` once
 * the last condition is removed. */
export function ConditionList({ base, condition, set, ...props }: PanelProps & {
  base: FieldAddress; condition: Predicate | undefined; set: (condition: Predicate | undefined, array?: ArrayEdit) => void;
}) {
  const tx = useTranslation(), [picking, setPicking] = useState<number>();
  const conditions = condition ? conditionsOf(condition) : [], grouped = condition?.type === 'and' || condition?.type === 'or';
  const operator = condition?.type === 'or' ? 'or' : 'and';
  return <>
    <div className={clsx(css.branchConditionList, conditions.length > 1 && css.branchConditionGroup)}>
      {conditions.length > 1 && <div className={css.branchLogicBracket}>
        <button type="button" className={css.branchLogicToggle} aria-label={tx('workflows:branch.match')} title={tx(operator === 'or' ? 'workflows:branch.or' : 'workflows:branch.and')}
          onClick={() => set({ type: operator === 'or' ? 'and' : 'or', predicates: [...conditions] })}>
          {operator === 'or' ? /* i18n-raw: logical operator token, as Dify shows it */ 'OR' : /* i18n-raw: logical operator token, as Dify shows it */ 'AND'}<IconRefreshOutline14 />
        </button>
      </div>}
      {conditions.map((item, position) => {
        const address = grouped ? within(base, ['predicates', position]) : base;
        return <div className={css.branchConditionRow} key={position}>
          <ConditionRow {...props} address={address} condition={item} initiallyOpen={picking === position} onPicked={() => setPicking(undefined)} />
          <button type="button" className={css.branchRemoveCondition} aria-label={tx('workflows:branch.remove-condition', { p0: String(position + 1) })} onClick={() => {
            const remaining = conditions.filter((_, i) => i !== position);
            set(remaining.length ? { type: operator, predicates: remaining } : undefined, grouped ? { array: within(base, ['predicates']), removed: position } : undefined);
          }}><IconTrashOutline16 /></button>
        </div>;
      })}
    </div>
    <button type="button" className={css.branchAddCondition} onClick={() => {
      const position = conditions.length;
      set({ type: operator, predicates: [...conditions, unfinished()] }, grouped ? { array: within(base, ['predicates']), inserted: position } : undefined);
      setPicking(position);
    }}><IconPlus size={13} />{tx('workflows:branch.add-condition')}</button>
  </>;
}

function ConditionRow({ address, condition, edit, initiallyOpen, onPicked }: PanelProps & {
  address: FieldAddress; condition: Predicate; initiallyOpen: boolean; onPicked: () => void;
}) {
  const tx = useTranslation();
  if (condition.type !== 'boolean' && condition.type !== 'equal' && condition.type !== 'not_equal' && condition.type !== 'contains'
    && condition.type !== 'not_contains' && condition.type !== 'starts_with' && condition.type !== 'ends_with'
    && condition.type !== 'empty' && condition.type !== 'not_empty')
    return <PredicateEditor label={tx('workflows:branch.condition')} address={address} predicate={condition} edit={edit} />;
  const comparable = (value: Value) => value.type === 'reference' || (value.type === 'literal' && (value.value === null || typeof value.value === 'boolean' || typeof value.value === 'string'));
  const unary = condition.type === 'boolean' || condition.type === 'empty' || condition.type === 'not_empty';
  const left = unary ? condition.value : condition.left;
  const right: Value = unary ? { type: 'literal', value: condition.type === 'boolean' ? true : '' } : condition.right;
  const simple = left.type === 'reference' && (unary || comparable(right));
  if (!simple) return <PredicateEditor label={tx('workflows:branch.condition')} address={address} predicate={condition} edit={edit} />;
  const string = condition.type === 'contains' || condition.type === 'not_contains' || condition.type === 'starts_with'
    || condition.type === 'ends_with' || condition.type === 'empty' || condition.type === 'not_empty'
    || right.type === 'literal' && typeof right.value === 'string';
  const operator = condition.type === 'boolean' ? 'equal' : condition.type;
  const options: readonly SimpleOperator[] = string ? stringOperators : ['equal', 'not_equal'];
  const valueFree = operator === 'empty' || operator === 'not_empty';
  const replace = (predicate: Predicate) => edit(document => setField(document, address, predicate));
  const setOperator = (type: SimpleOperator) => {
    if (type === 'empty' || type === 'not_empty') replace({ type, value: left });
    else if (type === 'equal' || type === 'not_equal') replace({ type, left, right });
    else replace({ type, left, right: right.type === 'literal' && typeof right.value === 'string' ? right : { type: 'literal', value: '' } });
  };
  const picked = (choice: WorkflowReferenceChoice) => {
    const left: Value = { type: 'reference', path: choice.path };
    if (choice.schema?.type === 'string') replace({ type: 'contains', left, right: { type: 'literal', value: '' } });
    else replace({ type: 'equal', left, right: { type: 'literal', value: choice.schema?.type === 'boolean' ? true : choice.schema?.type === 'null' ? null : '' } });
    onPicked();
  };
  return <div className={css.branchConditionControls}>
    <div className={css.branchConditionTop}>
      <ReferencePicker address={within(address, [unary ? 'value' : 'left'])} path={left.type === 'reference' ? left.path : []}
        edit={edit} token initiallyOpen={initiallyOpen} onChoose={picked} placeholder={tx('workflows:branch.select-variable')} />
      <span className={css.branchOperatorDivider} aria-hidden />
      <Menu.Root modal={false}>
        <Menu.Trigger className={css.branchOperator} aria-label={tx('workflows:branch.operator')} disabled={left.type !== 'reference' || !left.path.length}>
          {tx(`workflows:branch.${operator}`)}<IconChevronDownOutline14 />
        </Menu.Trigger>
        <Menu.Portal><Menu.Positioner side="bottom" align="end" sideOffset={4} className={css.branchOperatorPositioner}>
          <Menu.Popup className={css.branchOperatorPopup}>
            {options.map(type => <Menu.Item key={type} className={css.branchOperatorItem} onClick={() => setOperator(type)}>{tx(`workflows:branch.${type}`)}</Menu.Item>)}
          </Menu.Popup>
        </Menu.Positioner></Menu.Portal>
      </Menu.Root>
    </div>
    {!valueFree && <div className={css.branchConditionValue}>
      {right.type === 'reference' ? <ReferencePicker address={within(address, ['right'])} path={right.path} edit={edit} />
        : right.type === 'literal' && typeof right.value === 'boolean' ? <select className={css.input} aria-label={tx('workflows:branch.value')} value={String(right.value)} onChange={event => replace({ type: operator as 'equal' | 'not_equal', left, right: { type: 'literal', value: event.target.value === 'true' } })}><option value="true">{tx('workflows:port.true')}</option><option value="false">{tx('workflows:port.false')}</option></select>
          : right.type === 'literal' && typeof right.value === 'string' ? <DraftTextField label={tx('workflows:branch.value')} value={right.value} address={within(address, ['right', 'value'])} placeholder={tx('workflows:branch.value-placeholder')} />
            : <span className={css.branchNull}>null</span>}
    </div>}
  </div>;
}
