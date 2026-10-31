import { Menu } from '@base-ui/react/menu';
import type { WorkflowAssignment, WorkflowLoopVariable, WorkflowVarType, WorkflowWriteMode } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { IconChevronDownOutline14, IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { setField, within, type FieldAddress } from '../../../document/fields';
import { blockAt, nodeAt, type Value } from '../../../document/model';
import { IconPlus } from '../../icons';
import { ReferencePicker } from '../forms/ReferencePicker';
import { DraftTextField, JsonField, Section } from '../forms/fields';
import { ConditionList } from './Conditions';
import { SliderField } from './InferencePanels';
import { OutputDeclaration, useNode, type PanelProps } from './shared';
import css from '../Editor.module.css';

/** Dify's `LOOP_NODE_MAX_COUNT` and `MAX_PARALLEL_LIMIT`. */
const LOOP_MAX_COUNT = 100, PARALLEL_LIMIT = 10;
const VAR_TYPES: readonly WorkflowVarType[] = ['string', 'number', 'boolean', 'object', 'array[string]', 'array[number]', 'array[boolean]', 'array[object]'];
const ITERATION_ERRORS = ['terminated', 'continue_on_error', 'remove_abnormal_output'] as const;
const unset = (): Value => ({ type: 'reference', path: [] });

/** A constant of a type, as Dify starts one. */
function constant(type: WorkflowVarType): Value {
  const value = type === 'string' ? '' : type === 'number' ? 0 : type === 'boolean' ? false : type === 'object' ? {} : [];
  return { type: 'literal', value };
}

/** A literal's editor by type: text, true/false, or JSON. */
function ConstantField({ label, type, value, address, edit }: { label: string; type: WorkflowVarType | 'number'; value: Value; address: FieldAddress; edit: PanelProps['edit'] }) {
  const tx = useTranslation();
  if (value.type !== 'literal') return null;
  if (type === 'string' && typeof value.value === 'string') return <DraftTextField label={label} value={value.value} address={within(address, ['value'])} />;
  if (type === 'boolean') return <label className={css.inlineField}><span>{label}</span>
    <select className={css.input} aria-label={label} value={String(value.value === true)} onChange={event => edit(document => setField(document, address, { type: 'literal', value: event.target.value === 'true' }))}>
      <option value="true">{tx('workflows:port.true')}</option><option value="false">{tx('workflows:port.false')}</option>
    </select></label>;
  return <JsonField label={label} value={value.value} rows={3} line={type === 'number'} address={within(address, ['value'])} />;
}


/** As Dify's Loop panel: loop variables, the termination condition and the
 * maximum loop count. The Loop's value is its variables. */
export function LoopPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'loop');
  const variables = node.loop_variables ?? [];
  // An empty list and an absent condition are omitted, as native writes them.
  const setVariables = (next: WorkflowLoopVariable[]) => update(({ loop_variables: _, ...current }) => next.length ? { ...current, loop_variables: next } : current, at('loop_variables'));
  const add = () => {
    // A variable's label is an identifier, never copy.
    const label = (index: number) => /* i18n-raw: native variable identifier */ `var_${index}`;
    let index = variables.length + 1; while (variables.some(item => item.label === label(index))) index++;
    setVariables([...variables, { label: label(index), var_type: 'string', value: constant('string') }]);
  };
  return <>
    <Section title={tx('workflows:loop.variables')} aside={<button type="button" className={css.iconButton} aria-label={tx('workflows:loop.add-variable')} onClick={add}><IconPlus size={14} /></button>}>
      {variables.length ? variables.map((variable, index) => {
        const base = at('loop_variables', index), variableMode = variable.value.type === 'reference';
        const set = (change: Partial<WorkflowLoopVariable>) => setVariables(variables.map((item, i) => i === index ? { ...item, ...change } : item));
        return <div key={index} className={css.loopVariable}>
          <div className={css.loopVariableHead}>
            <DraftTextField label={tx('workflows:loop.variable-name')} value={variable.label} address={within(base, ['label'])} compact />
            <select className={css.input} aria-label={tx('workflows:loop.variable-type')} value={variable.var_type} onChange={event => { const var_type = event.target.value as WorkflowVarType; set({ var_type, value: variableMode ? unset() : constant(var_type) }); }}>
              {VAR_TYPES.map(type => <option key={type} value={type}>{type}</option>)}
            </select>
            <select className={css.input} aria-label={tx('workflows:loop.input-mode')} value={variableMode ? 'variable' : 'constant'} onChange={event => set({ value: event.target.value === 'variable' ? unset() : constant(variable.var_type) })}>
              <option value="variable">{tx('workflows:loop.mode-variable')}</option><option value="constant">{tx('workflows:loop.mode-constant')}</option>
            </select>
            <button type="button" className={css.iconButton} aria-label={tx('workflows:loop.remove-variable', { p0: variable.label })} onClick={() => setVariables(variables.filter((_, i) => i !== index))}><IconTrashOutline16 /></button>
          </div>
          {variableMode && variable.value.type === 'reference'
            ? <ReferencePicker address={within(base, ['value'])} path={variable.value.path} edit={props.edit} token placeholder={tx('workflows:branch.select-variable')} />
            : <ConstantField label={tx('workflows:loop.value')} type={variable.var_type} value={variable.value} address={within(base, ['value'])} edit={props.edit} />}
        </div>;
      }) : <p className={css.note}>{tx('workflows:loop.no-variables')}</p>}
    </Section>
    <Section title={tx('workflows:loop.break-condition')}>
      <p className={css.note}>{tx('workflows:loop.break-condition-tip')}</p>
      <ConditionList {...props} base={at('break_condition')} condition={node.break_condition ?? undefined}
        set={(condition, array) => update(({ break_condition: _, ...current }) => condition ? { ...current, break_condition: condition } : current, at('break_condition'), array)} />
    </Section>
    <Section title={tx('workflows:loop.max-count')}>
      <SliderField label={tx('workflows:loop.max-count')} unit={tx('workflows:loop.times')} min={1} max={LOOP_MAX_COUNT} step={1} value={node.loop_count}
        onChange={loop_count => update(current => ({ ...current, loop_count }), at('loop_count'))} />
    </Section>
    <OutputDeclaration {...props} />
  </>;
}

/** As Dify's Iteration panel: the array to iterate, the output variable,
 * parallel mode, the error response method and output flattening. */
export function IterationPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'iteration');
  const mode = node.error_handle_mode ?? 'terminated', flatten = node.flatten_output ?? true;
  const badge = <span className={css.typeBadge}>{/* i18n-raw: Dify's type badge */ 'Array'}</span>;
  return <>
    <Section title={tx('workflows:iteration.input')} aside={badge}>
      <ReferencePicker address={at('iterator')} path={node.iterator.type === 'reference' ? node.iterator.path : []} edit={props.edit} token placeholder={tx('workflows:branch.select-variable')} />
    </Section>
    <Section title={tx('workflows:iteration.output')} aside={badge}>
      <ReferencePicker address={at('output')} path={node.output.type === 'reference' ? node.output.path : []} edit={props.edit} token placeholder={tx('workflows:branch.select-variable')} />
    </Section>
    <section className={css.group} aria-label={tx('workflows:iteration.parallel')}>
      <h3 className={css.groupHeading}><span className={css.headingText}>{tx('workflows:iteration.parallel')}</span>
        <Switch checked={!!node.is_parallel} label={tx('workflows:iteration.parallel')} onChange={on => update(({ is_parallel: _, ...current }) => on ? { ...current, is_parallel: true } : current, at('is_parallel'))} /></h3>
      <p className={css.note}>{tx('workflows:iteration.parallel-desc')}</p>
      {node.is_parallel && <SliderField label={tx('workflows:iteration.max-parallelism')} min={1} max={PARALLEL_LIMIT} step={1} value={node.parallel_nums ?? 10}
        onChange={parallel_nums => update(current => ({ ...current, parallel_nums }), at('parallel_nums'))} />}
    </section>
    <section className={css.group} aria-label={tx('workflows:iteration.error-method')}>
      <h3 className={css.groupHeading}><span className={css.headingText}>{tx('workflows:iteration.error-method')}</span>
        <Menu.Root modal={false}>
          <Menu.Trigger className={css.strategyTrigger} aria-label={tx('workflows:iteration.error-method')}>{tx(`workflows:iteration.error-${mode}`)}<IconChevronDownOutline14 /></Menu.Trigger>
          <Menu.Portal><Menu.Positioner side="bottom" align="end" sideOffset={4} className={css.branchOperatorPositioner}>
            <Menu.Popup className={css.strategyPopup}>
              {ITERATION_ERRORS.map(type => <Menu.Item key={type} className={css.strategyItem} onClick={() => update(({ error_handle_mode: _, ...current }) => type === 'terminated' ? current : { ...current, error_handle_mode: type }, at('error_handle_mode'))}>
                <b>{tx(`workflows:iteration.error-${type}`)}</b>
              </Menu.Item>)}
            </Menu.Popup>
          </Menu.Positioner></Menu.Portal>
        </Menu.Root>
      </h3>
    </section>
    <section className={css.group} aria-label={tx('workflows:iteration.flatten')}>
      <h3 className={css.groupHeading}><span className={css.headingText}>{tx('workflows:iteration.flatten')}</span>
        <Switch checked={flatten} label={tx('workflows:iteration.flatten')} onChange={on => update(({ flatten_output: _, ...current }) => on ? current : { ...current, flatten_output: false }, at('flatten_output'))} /></h3>
      <p className={css.note}>{tx('workflows:iteration.flatten-desc')}</p>
    </section>
    <OutputDeclaration {...props} />
  </>;
}

/** As Dify's Exit Loop: no configuration. */
export function LoopEndPanel() {
  const tx = useTranslation();
  return <Section title={tx('workflows:kind.loop_end')}><p className={css.note}>{tx('workflows:kind-about.loop_end')}</p></Section>;
}

/** Dify's write modes a variable type offers, in its menu order. */
function operations(type: WorkflowVarType): WorkflowWriteMode[] {
  if (type.startsWith('array')) return ['overwrite', 'clear', 'append', 'extend', 'remove_first', 'remove_last'];
  if (type === 'number') return ['overwrite', 'clear', 'set', 'increment', 'decrement', 'multiply', 'divide'];
  return ['overwrite', 'clear', 'set'];
}

/** The operand a write mode starts with: a variable, a constant, or none. */
function operand(mode: WorkflowWriteMode, type: WorkflowVarType): Value | undefined {
  if (mode === 'clear' || mode === 'remove_first' || mode === 'remove_last') return undefined;
  if (mode === 'set') return constant(type);
  if (mode === 'increment' || mode === 'decrement' || mode === 'multiply' || mode === 'divide') return { type: 'literal', value: 1 };
  return unset();
}

/** As Dify's Variable Assigner: each item writes one variable of the Loop
 * the node is in, by an operation its type offers. */
export function AssignerPanel(props: PanelProps) {
  const tx = useTranslation(), { node, at, update } = useNode(props, 'assigner');
  const step = props.block.at(-1), owner = step && nodeAt(blockAt(props.document, props.block.slice(0, -1))!, step.node);
  const variables = owner?.type === 'loop' ? owner.loop_variables ?? [] : [];
  const setItems = (items: WorkflowAssignment[]) => update(current => ({ ...current, items }), at('items'));
  const typeOf = (item: WorkflowAssignment) => item.variable[0] === step?.node ? variables.find(variable => variable.label === item.variable[1])?.var_type : undefined;
  if (owner?.type !== 'loop') return <Section title={tx('workflows:assigner.variables')}><p className={css.note}>{tx('workflows:assigner.outside-loop')}</p></Section>;
  return <Section title={tx('workflows:assigner.variables')} aside={<button type="button" className={css.iconButton} aria-label={tx('workflows:assigner.add')} disabled={!variables.length}
    onClick={() => { const first = variables[0]; setItems([...node.items, { variable: [step!.node, first.label], operation: 'overwrite', value: unset() }]); }}><IconPlus size={14} /></button>}>
    {!variables.length && <p className={css.note}>{tx('workflows:assigner.no-loop-variables')}</p>}
    {node.items.length ? node.items.map((item, index) => {
      const type = typeOf(item), base = at('items', index);
      const set = (variable: string[], operation: WorkflowWriteMode, value: Value | undefined) =>
        setItems(node.items.map((current, i) => i === index ? { variable, operation, ...(value ? { value } : {}) } : current));
      return <div key={index} className={css.loopVariable}>
        <div className={css.loopVariableHead}>
          <select className={css.input} aria-label={tx('workflows:assigner.variable')} value={item.variable[1] ?? ''} onChange={event => {
            const variable = variables.find(entry => entry.label === event.target.value)!;
            set([step!.node, variable.label], 'overwrite', unset());
          }}>
            {!type && <option value="">{tx('workflows:assigner.select')}</option>}
            {variables.map(variable => <option key={variable.label} value={variable.label}>{variable.label} · {variable.var_type}</option>)}
          </select>
          <select className={css.input} aria-label={tx('workflows:assigner.operation')} value={item.operation} disabled={!type} onChange={event => {
            const mode = event.target.value as WorkflowWriteMode;
            set(item.variable, mode, type && operand(mode, type));
          }}>
            {(type ? operations(type) : [item.operation]).map(mode => <option key={mode} value={mode}>{tx(`workflows:assigner.op-${mode}`)}</option>)}
          </select>
          <button type="button" className={css.iconButton} aria-label={tx('workflows:assigner.remove', { p0: String(index + 1) })} onClick={() => setItems(node.items.filter((_, i) => i !== index))}><IconTrashOutline16 /></button>
        </div>
        {item.value?.type === 'reference' ? <ReferencePicker address={within(base, ['value'])} path={item.value.path} edit={props.edit} token placeholder={tx('workflows:branch.select-variable')} />
          : item.value && type ? <ConstantField label={tx('workflows:loop.value')} type={item.operation === 'set' ? type : 'number'} value={item.value} address={within(base, ['value'])} edit={props.edit} /> : null}
      </div>;
    }) : variables.length ? <p className={css.note}>{tx('workflows:assigner.empty')}</p> : null}
  </Section>;
}
