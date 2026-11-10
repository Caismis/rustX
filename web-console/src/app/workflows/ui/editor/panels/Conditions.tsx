import { useState } from 'react';
import { Menu } from '@base-ui/react/menu';
import clsx from 'clsx';
import type { WorkflowReferenceChoice } from '../../../../../../../protocol/app-server/v78';
import { useTranslation } from '../../../../../locale/react';
import {
  IconChevronDownOutline14,
  IconRefreshOutline14,
  IconTrashOutline16,
} from '../../../../../presentation/primitives/icons';
import { conditionsOf } from '../../../document/branch';
import type { Predicate, Value } from '../../../document/model';
import { setField, within, type ArrayEdit, type FieldAddress } from '../../../document/fields';
import { IconPlus } from '../../icons';
import { PredicateEditor } from '../forms/ValueEditor';
import { ReferencePicker } from '../forms/ReferencePicker';
import { JsonField } from '../forms/fields';
import { TemplateInput, templateParts, templateValue } from '../forms/TemplateInput';
import { useReferenceSchema } from '../forms/useReferenceSchema';
import { comparisonSymbols } from '../summary';
import type { PanelProps } from './shared';
import css from '../Editor.module.css';

const unfinished = (): Predicate => ({
  type: 'equal',
  left: { type: 'reference', path: [] },
  right: { type: 'reference', path: [] },
});
const stringOperators = [
  'contains',
  'not_contains',
  'starts_with',
  'ends_with',
  'equal',
  'not_equal',
  'empty',
  'not_empty',
] as const;
const numberOperators = [
  'equal',
  'not_equal',
  'greater',
  'less',
  'greater_equal',
  'less_equal',
  'empty',
  'not_empty',
] as const;
type SimpleOperator = (typeof stringOperators)[number] | (typeof numberOperators)[number];
type Kind = 'string' | 'number' | 'boolean' | 'array' | 'object' | 'null' | 'unknown';
function kindOf(schema?: Record<string, unknown>): Kind {
  const type = schema?.type;
  return type === 'integer'
    ? 'number'
    : ['string', 'number', 'boolean', 'array', 'object', 'null'].includes(String(type))
      ? (type as Kind)
      : 'unknown';
}
const constant = (kind: Kind): Value => ({
  type: 'literal',
  value: kind === 'number' ? 0 : kind === 'boolean' ? true : kind === 'null' ? null : '',
});

export function ConditionList({
  base,
  condition,
  set,
  ...props
}: PanelProps & {
  base: FieldAddress;
  condition: Predicate | undefined;
  set: (condition: Predicate | undefined, array?: ArrayEdit) => void;
}) {
  const tx = useTranslation(),
    [picking, setPicking] = useState<number>();
  const conditions = condition ? conditionsOf(condition) : [],
    grouped = condition?.type === 'and' || condition?.type === 'or',
    operator = condition?.type === 'or' ? 'or' : 'and';
  return (
    <>
      <div className={clsx(css.branchConditionList, conditions.length > 1 && css.branchConditionGroup)}>
        {conditions.length > 1 && (
          <div className={css.branchLogicBracket}>
            <button
              type="button"
              className={css.branchLogicToggle}
              aria-label={tx('workflows:branch.match')}
              title={tx(operator === 'or' ? 'workflows:branch.or' : 'workflows:branch.and')}
              onClick={() => set({ type: operator === 'or' ? 'and' : 'or', predicates: [...conditions] })}
            >
              {operator === 'or'
                ? /* i18n-raw: logical operator token */ 'OR'
                : /* i18n-raw: logical operator token */ 'AND'}
              <IconRefreshOutline14 />
            </button>
          </div>
        )}
        {conditions.map((item, position) => (
          <div className={css.branchConditionRow} key={position}>
            <ConditionRow
              {...props}
              address={grouped ? within(base, ['predicates', position]) : base}
              condition={item}
              initiallyOpen={picking === position}
              onPicked={() => setPicking(undefined)}
            />
            <button
              type="button"
              className={css.branchRemoveCondition}
              aria-label={tx('workflows:branch.remove-condition', { p0: String(position + 1) })}
              onClick={() => {
                const remaining = conditions.filter((_, i) => i !== position);
                set(
                  remaining.length ? { type: operator, predicates: remaining } : undefined,
                  grouped ? { array: within(base, ['predicates']), removed: position } : undefined,
                );
              }}
            >
              <IconTrashOutline16 />
            </button>
          </div>
        ))}
      </div>
      <button
        type="button"
        className={css.branchAddCondition}
        onClick={() => {
          const position = conditions.length;
          set(
            { type: operator, predicates: [...conditions, unfinished()] },
            grouped ? { array: within(base, ['predicates']), inserted: position } : undefined,
          );
          setPicking(position);
        }}
      >
        <IconPlus size={13} />
        {tx('workflows:branch.add-condition')}
      </button>
    </>
  );
}
function ConditionRow(
  props: PanelProps & { address: FieldAddress; condition: Predicate; initiallyOpen: boolean; onPicked: () => void },
) {
  const tx = useTranslation(),
    { condition, address } = props;
  if (condition.type === 'and' || condition.type === 'or' || condition.type === 'not' || condition.type === 'boolean')
    return (
      <PredicateEditor
        label={tx('workflows:branch.condition')}
        address={address}
        predicate={condition}
        edit={props.edit}
      />
    );
  const unary = 'value' in condition,
    left = unary ? condition.value : condition.left;
  if (left.type !== 'reference')
    return (
      <PredicateEditor
        label={tx('workflows:branch.condition')}
        address={address}
        predicate={condition}
        edit={props.edit}
      />
    );
  return <SimpleCondition {...props} condition={condition} left={left} unary={unary} />;
}
function SimpleCondition({
  condition,
  address,
  edit,
  left,
  unary,
  initiallyOpen,
  onPicked,
}: PanelProps & {
  address: FieldAddress;
  condition: Exclude<Predicate, { type: 'and' | 'or' | 'not' | 'boolean' }>;
  left: Extract<Value, { type: 'reference' }>;
  unary: boolean;
  initiallyOpen: boolean;
  onPicked: () => void;
}) {
  const tx = useTranslation(),
    location = within(address, [unary ? 'value' : 'left']),
    schema = useReferenceSchema(location, left.path, address);
  const right: Value = 'right' in condition ? condition.right : constant('string');
  const inferred =
    right.type === 'literal'
      ? typeof right.value === 'number'
        ? 'number'
        : typeof right.value === 'boolean'
          ? 'boolean'
          : typeof right.value === 'string'
            ? 'string'
            : 'null'
      : 'unknown';
  const kind = kindOf(schema) === 'unknown' ? inferred : kindOf(schema),
    itemKind = kind === 'array' ? kindOf(schema?.items as Record<string, unknown>) : kind;
  const operator = condition.type;
  const options: readonly SimpleOperator[] =
    kind === 'number'
      ? numberOperators
      : kind === 'boolean'
        ? ['equal', 'not_equal']
        : kind === 'array'
          ? itemKind === 'object' || itemKind === 'unknown'
            ? ['empty', 'not_empty']
            : ['contains', 'not_contains', 'empty', 'not_empty']
          : kind === 'object' || kind === 'null'
            ? ['empty', 'not_empty']
            : stringOperators;
  const valueFree = operator === 'empty' || operator === 'not_empty';
  // The compact text input only understands flat string parts; the other
  // compact controls edit one scalar or reference. Keep the native editor
  // for every predicate or value these controls cannot show without loss.
  const simpleRight =
    right.type === 'reference' ||
    (itemKind === 'string' &&
      ((right.type === 'literal' && typeof right.value === 'string') ||
        (right.type === 'template' && right.parts.every(part =>
          part.type === 'reference' || part.type === 'literal' && typeof part.value === 'string')))) ||
    (right.type === 'literal' &&
      ((itemKind === 'number' && typeof right.value === 'number') ||
        (itemKind === 'boolean' && typeof right.value === 'boolean')));
  if (!options.includes(operator) || (!valueFree && !simpleRight))
    return <PredicateEditor label={tx('workflows:branch.condition')} address={address} predicate={condition} edit={edit} />;
  const replace = (predicate: Predicate) => edit((document) => setField(document, address, predicate));
  const binary = (type: Exclude<SimpleOperator, 'empty' | 'not_empty'>, value: Value) =>
    replace({ type, left, right: value });
  const setOperator = (type: SimpleOperator) => {
    if (type === 'empty' || type === 'not_empty') replace({ type, value: left });
    else binary(type, 'right' in condition ? right : constant(itemKind));
  };
  const picked = (choice: WorkflowReferenceChoice) => {
    const left: Value = { type: 'reference', path: choice.path },
      kind = kindOf(choice.schema as Record<string, unknown>);
    replace(
      kind === 'array' || kind === 'object' || kind === 'null'
        ? { type: 'empty', value: left }
        : { type: kind === 'string' ? 'contains' : 'equal', left, right: constant(kind) },
    );
    onPicked();
  };
  return (
    <div className={css.branchConditionControls}>
      <div className={css.branchConditionTop}>
        <ReferencePicker
          address={location}
          path={left.path}
          edit={edit}
          token
          initiallyOpen={initiallyOpen}
          onChoose={picked}
          placeholder={tx('workflows:branch.select-variable')}
        />
        <span className={css.branchOperatorDivider} aria-hidden />
        <Menu.Root modal={false}>
          <Menu.Trigger
            className={css.branchOperator}
            aria-label={tx('workflows:branch.operator')}
            title={tx(`workflows:branch.${operator}`)}
            disabled={!left.path.length}
          >
            {(kind === 'number' && comparisonSymbols[operator]) || tx(`workflows:branch.${operator}`)}
            <IconChevronDownOutline14 />
          </Menu.Trigger>
          <Menu.Portal>
            <Menu.Positioner side="bottom" align="end" sideOffset={4} className={css.branchOperatorPositioner}>
              <Menu.Popup className={css.branchOperatorPopup}>
                {options.map((type) => (
                  <Menu.Item key={type} className={css.branchOperatorItem} onClick={() => setOperator(type)}>
                    {tx(`workflows:branch.${type}`)}
                  </Menu.Item>
                ))}
              </Menu.Popup>
            </Menu.Positioner>
          </Menu.Portal>
        </Menu.Root>
      </div>
      {!valueFree && (
        <div className={css.branchConditionValue}>
          {itemKind === 'string' ? (
            <TemplateInput
              parts={templateParts(right)}
              address={within(address, ['right'])}
              edit={edit}
              label={tx('workflows:branch.value')}
              onChange={(parts) =>
                binary(
                  operator as Exclude<SimpleOperator, 'empty' | 'not_empty'>,
                  templateValue(parts) ?? { type: 'template', parts: [] },
                )
              }
            />
          ) : (
            <>
              {(itemKind === 'number' || itemKind === 'boolean') && (
                <div className={css.operandMode} role="radiogroup" aria-label={tx('workflows:loop.input-mode')}>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={right.type !== 'reference'}
                    onClick={() =>
                      binary(operator as Exclude<SimpleOperator, 'empty' | 'not_empty'>, constant(itemKind))
                    }
                  >
                    {tx('workflows:loop.mode-constant')}
                  </button>
                  <button
                    type="button"
                    role="radio"
                    aria-checked={right.type === 'reference'}
                    onClick={() =>
                      binary(operator as Exclude<SimpleOperator, 'empty' | 'not_empty'>, {
                        type: 'reference',
                        path: [],
                      })
                    }
                  >
                    {tx('workflows:loop.mode-variable')}
                  </button>
                </div>
              )}
              {right.type === 'reference' ? (
                <ReferencePicker address={within(address, ['right'])} path={right.path} edit={edit} token />
              ) : right.type === 'literal' && typeof right.value === 'number' ? (
                <JsonField
                  label={tx('workflows:branch.value')}
                  value={right.value}
                  address={within(address, ['right', 'value'])}
                  line
                />
              ) : right.type === 'literal' && typeof right.value === 'boolean' ? (
                <select
                  className={css.input}
                  aria-label={tx('workflows:branch.value')}
                  value={String(right.value)}
                  onChange={(event) =>
                    binary(operator as Exclude<SimpleOperator, 'empty' | 'not_empty'>, {
                      type: 'literal',
                      value: event.target.value === 'true',
                    })
                  }
                >
                  <option value="true">{tx('workflows:port.true')}</option>
                  <option value="false">{tx('workflows:port.false')}</option>
                </select>
              ) : null}
            </>
          )}
        </div>
      )}
    </div>
  );
}
