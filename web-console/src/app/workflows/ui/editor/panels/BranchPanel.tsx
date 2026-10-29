import { useState } from 'react';
import { Menu } from '@base-ui/react/menu';
import clsx from 'clsx';
import type { WorkflowReferenceChoice } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { IconChevronDownOutline14, IconRefreshOutline14, IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { addBranchCase, removeBranchCase } from '../../../document/commands';
import { conditionsOf } from '../../../document/branch';
import { blockAt, nodeAt, type Predicate, type Value } from '../../../document/model';
import { setField, within, type ArrayEdit, type FieldAddress } from '../../../document/fields';
import { IconPlus } from '../../icons';
import { PredicateEditor } from '../forms/ValueEditor';
import { ReferencePicker } from '../forms/ReferencePicker';
import { DraftTextField } from '../forms/fields';
import { useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

const unfinished = (): Predicate => ({ type: 'equal', left: { type: 'reference', path: [] }, right: { type: 'reference', path: [] } });
const stringOperators = ['contains', 'not_contains', 'starts_with', 'ends_with', 'equal', 'not_equal', 'empty', 'not_empty'] as const;
type SimpleOperator = typeof stringOperators[number];

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
        <div className={css.branchCaseName}><b>{tx(index ? 'workflows:branch.elif' : 'workflows:branch.if')}</b>{node.cases.length > 1 && <small>{tx('workflows:branch.case', { p0: String(index + 1) })}</small>}</div>
        <div className={css.branchCaseContent}>
          {advanced ? <PredicateEditor label={tx('workflows:branch.case', { p0: String(index + 1) })} address={base} predicate={item.condition} edit={props.edit} /> : <>
            <div className={clsx(css.branchConditionList, conditions.length > 1 && css.branchConditionGroup)}>
              {/* As Dify's ConditionList: a bracket joins the conditions and its
                  button toggles AND/OR. */}
              {conditions.length > 1 && <div className={css.branchLogicBracket}>
                <button type="button" className={css.branchLogicToggle} aria-label={tx('workflows:branch.match')} title={tx(item.condition.type === 'or' ? 'workflows:branch.or' : 'workflows:branch.and')}
                  onClick={() => change(item.id, { type: item.condition.type === 'or' ? 'and' : 'or', predicates: [...conditions] })}>
                  {/* i18n-raw: logical operator token, as Dify shows it in every locale */ item.condition.type === 'or' ? 'OR' : 'AND'}<IconRefreshOutline14 />
                </button>
              </div>}
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
                  }}><IconTrashOutline16 /></button>
                </div>;
              })}
            </div>
          </>}
          <button type="button" className={css.branchAddCondition} onClick={() => {
            const position = conditions.length;
            change(item.id, { type: item.condition.type === 'or' ? 'or' : 'and', predicates: [...conditions, unfinished()] }, grouped ? { array: within(base, ['predicates']), inserted: position } : undefined);
            setPicking(`${item.id}:${position}`);
          }}><IconPlus size={13} />{tx('workflows:branch.add-condition')}</button>
        </div>
        {node.cases.length > 1 && <button type="button" className={css.branchRemoveCase} aria-label={tx('workflows:branch.remove-case', { p0: tx('workflows:branch.case', { p0: String(index + 1) }) })}
          onClick={() => props.edit(document => removeBranchCase(document, props.block, props.id, item.id), undefined, { array: at('cases'), removed: index })}><IconTrashOutline16 /></button>}
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
        edit={edit} conditionVariable initiallyOpen={initiallyOpen} onChoose={picked} placeholder={tx('workflows:branch.select-variable')} />
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
