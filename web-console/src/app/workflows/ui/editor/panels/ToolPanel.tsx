import clsx from 'clsx';
import { useEffect, useState } from 'react';
import type { WorkflowToolChoice } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { IconChevronDownOutline14 } from '../../../../../presentation/primitives/icons';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { encodeField } from '../../../document/fields';
import { blockAt, nodeAt, type Value } from '../../../document/model';
import { schemaObject } from '../../../document/schema';
import { ReferencePicker } from '../forms/ReferencePicker';
import { TemplateInput, templateParts, templateValue } from '../forms/TemplateInput';
import { ValueEditor } from '../forms/ValueEditor';
import { JsonField, Section, SelectField, TextField } from '../forms/fields';
import { CommandRejected, replaceNodeExits } from '../../../document/commands';
import { ErrorStrategy, RetrySettings, type Retry } from './InferencePanels';
import { useNode, type PanelProps } from './shared';
import { useToolCatalog } from '../toolCatalog';
import css from '../Editor.module.css';

type ToolNode = ReturnType<typeof useNode<'tool'>>;
type ArgumentKind = 'string' | 'enum' | 'integer' | 'number' | 'boolean' | 'json';

/** Arguments as one field per parameter: constructed fields, or the fields
 * of a literal object. Any other value is edited as a whole. */
function argumentFields(value: Value): Record<string, Value> | undefined {
  if (value.type === 'object') return value.fields;
  if (value.type === 'literal' && value.value && typeof value.value === 'object' && !Array.isArray(value.value))
    return Object.fromEntries(Object.entries(value.value).map(([key, item]) => [key, { type: 'literal', value: item }]));
  return undefined;
}

/** The form control a parameter schema asks for, as Dify's tool form. */
function argumentKind(schema: Record<string, unknown>): ArgumentKind {
  const types = (Array.isArray(schema.type) ? schema.type : [schema.type]).filter(type => type !== 'null');
  if (types.length !== 1) return 'json';
  if (types[0] === 'string') return Array.isArray(schema.enum) && schema.enum.every(item => typeof item === 'string') ? 'enum' : 'string';
  return types[0] === 'integer' || types[0] === 'number' || types[0] === 'boolean' ? types[0] : 'json';
}

/** As Dify's tool node: the selected Tool's parameters as an input form,
 * then its output variables. A source Tool is described by the scope's
 * catalog; a Tool nothing describes is configured by name with a raw value. */
export function ToolPanel(props: PanelProps) {
  const tx = useTranslation(), tool = useNode(props, 'tool'), { node } = tool, catalog = useToolCatalog();
  const { selector } = node, source = selector.origin === 'source' ? selector.source_id : undefined;
  useEffect(() => { if (source !== undefined && catalog.state.status === 'idle') catalog.load(); }, [source, catalog]);
  const choice = source === undefined ? props.vocabulary?.builtin_tools.find(item => item.name === selector.name)
    : catalog.state.status === 'ready' ? catalog.state.sources.find(item => item.source_id === source)?.tools.find(item => item.name === selector.name) : undefined;
  const fields = argumentFields(node.arguments);
  return <>
    {source !== undefined && catalog.state.status === 'loading' && <p className={css.note} role="status">{tx('workflows:tool.loading')}</p>}
    {!choice && <ToolSelection {...props} tool={tool} />}
    {choice && fields ? <ToolInputs {...props} tool={tool} choice={choice} fields={fields} />
      : <Section title={tx('workflows:tool.inputs')}><ValueEditor label={tx('workflows:inspector.tool-arguments')} address={tool.at('arguments')} value={node.arguments} edit={props.edit} /></Section>}
    <ToolOutputs {...props} tool={tool} />
    <ToolFailure {...props} tool={tool} />
  </>;
}

/** As Dify's tool node: retry on failure, then error handling. Each retry
 * is a new invocation, approved and recorded on its own. */
function ToolFailure(props: PanelProps & { tool: ToolNode }) {
  const tx = useTranslation(), { node, at, update } = props.tool, strategy = node.on_error ?? { type: 'stop' as const };
  const setRetry = (next?: Retry) => update(current => {
    const { retry: _previous, ...rest } = current;
    return next ? { ...rest, retry: next } : rest;
  }, at('retry'));
  const choose = (type: typeof strategy.type) => {
    if (type === strategy.type) return;
    props.edit(document => {
      const owner = blockAt(document, props.block), current = owner && nodeAt(owner, props.id);
      if (current?.type !== 'tool') throw new CommandRejected('missing_node');
      const { on_error: _previous, ...rest } = current;
      // A starting default of the result's own type, to be filled in.
      const next = type === 'stop' ? undefined : type === 'fail_branch' ? { type } : { type, value: current.result.type === 'text' ? '' : {} };
      // Without a fail branch the node has no fail port, so its connections go.
      return replaceNodeExits(document, props.block, props.id, next ? { ...rest, on_error: next } : rest);
    });
  };
  return <>
    <RetrySettings retry={node.retry} setRetry={setRetry} />
    <ErrorStrategy strategy={strategy.type} choose={choose} note={tx('workflows:failure.tool-retry')}>
      {strategy.type === 'default_value' && <div className={css.failureDefaults}>
        <JsonField label={tx('workflows:failure.default-value')} value={strategy.value} rows={4} address={at('on_error', 'value')} />
      </div>}
    </ErrorStrategy>
  </>;
}

function ToolSelection(props: PanelProps & { tool: ToolNode }) {
  const tx = useTranslation(), { node, at, update } = props.tool;
  return <Section title={tx('workflows:panel.tool-config')}>
    <SelectField label={tx('workflows:inspector.tool-origin')} value={node.selector.origin} options={[{ value: 'builtin', label: tx('workflows:inspector.tool-origin-builtin') }, { value: 'source', label: tx('workflows:inspector.tool-origin-source') }]}
      onChange={origin => { if (origin !== node.selector.origin) update(current => ({ ...current, selector: origin === 'builtin' ? { origin, name: current.selector.name } : { origin, source_id: '', name: current.selector.name } }), at('selector')); }} />
    {node.selector.origin === 'source' && <TextField label={tx('workflows:inspector.tool-source')} value={node.selector.source_id} invalid={!node.selector.source_id} field={encodeField(at('selector', 'source_id'))}
      onChange={(source_id, session) => update(current => ({ ...current, selector: { origin: 'source', source_id, name: current.selector.name } }), at('selector', 'source_id'), undefined, session)} />}
    {node.selector.origin === 'builtin'
      ? <SelectField label={tx('workflows:inspector.tool-name')} value={node.selector.name} options={[{ value: '', label: tx('workflows:tool.choose') }, ...(props.vocabulary?.builtin_tools ?? []).map(item => ({ value: item.name, label: item.name }))]}
        onChange={name => update(current => ({ ...current, selector: { origin: 'builtin', name } }), at('selector', 'name'))} />
      : <TextField label={tx('workflows:inspector.tool-name')} value={node.selector.name} invalid={!node.selector.name} field={encodeField(at('selector', 'name'))}
        onChange={(name, session) => update(current => ({ ...current, selector: { ...current.selector, name } }), at('selector', 'name'), undefined, session)} />}
  </Section>;
}

/** Dify's "Input variables": each parameter's name, requirement and
 * description over its control. An unset optional parameter is absent. */
function ToolInputs(props: PanelProps & { tool: ToolNode; choice: WorkflowToolChoice; fields: Record<string, Value> }) {
  const tx = useTranslation(), { at, update } = props.tool;
  const schema = schemaObject(props.choice.input_schema), properties = schemaObject(schema?.properties) ?? {};
  const required = Array.isArray(schema?.required) ? schema.required : [];
  const set = (name: string, value: Value | undefined, session?: string) => update(current => {
    const fields = { ...argumentFields(current.arguments) };
    if (value) fields[name] = value; else delete fields[name];
    return { ...current, arguments: { type: 'object', fields } };
  }, at('arguments', 'fields', { key: name }), undefined, session);
  return <section className={css.group} aria-label={tx('workflows:tool.inputs')}>
    <h3 className={css.groupHeading}>{tx('workflows:tool.inputs')}</h3>
    <div className={css.toolForm}>
      {Object.entries(properties).map(([name, value]) => <ToolArgument key={name} {...props} name={name} schema={schemaObject(value) ?? {}} required={required.includes(name)} value={props.fields[name]} set={set} />)}
      {!Object.keys(properties).length && <p className={css.note}>{tx('workflows:tool.no-inputs')}</p>}
    </div>
  </section>;
}

function ToolArgument({ name, schema, required, value, set, tool, edit }: PanelProps & {
  tool: ToolNode; name: string; schema: Record<string, unknown>; required: boolean; value?: Value; set: (name: string, value: Value | undefined, session?: string) => void;
}) {
  const tx = useTranslation(), address = tool.at('arguments', 'fields', { key: name }), kind = argumentKind(schema);
  const description = typeof schema.description === 'string' ? schema.description : undefined;
  const constant = value?.type === 'literal' ? value.value : undefined;
  const variable = value?.type === 'reference';
  return <div className={css.toolArgument}>
    <div className={css.toolArgumentHead}><b>{name}</b>{required && <span className={css.required} aria-label={tx('workflows:tool.required')}>*</span>}</div>
    {description && <p className={css.toolArgumentAbout}>{description}</p>}
    {kind === 'string' ? <TemplateInput parts={templateParts(value)} address={address} edit={edit} label={name} badge={tx('workflows:tool.type-string')} invalid={required && !value}
      onChange={(parts, session) => set(name, templateValue(parts), `${encodeField(address)}:${session}`)} />
      : <div className={css.toolArgumentRow}>
        {/* As Dify's FormInputTypeSwitch: a variable, or a constant. */}
        <div className={css.typeSwitch} role="radiogroup" aria-label={tx('workflows:value.kind')}>
          <button type="button" role="radio" aria-checked={variable} title={tx('workflows:tool.variable')} aria-label={tx('workflows:tool.variable')} className={clsx(variable && css.typeSwitchActive)}
            onClick={() => { if (!variable) set(name, { type: 'reference', path: [] }); }}>{/* i18n-raw: Dify's variable glyph */ '{x}'}</button>
          <button type="button" role="radio" aria-checked={!variable} title={tx('workflows:tool.constant')} aria-label={tx('workflows:tool.constant')} className={clsx(!variable && css.typeSwitchActive)}
            onClick={() => { if (variable) set(name, undefined); }}>{/* i18n-raw: a pencil, as Dify's constant glyph */ '✎'}</button>
        </div>
        {variable ? <ReferencePicker address={address} path={value.path} edit={edit} placeholder={tx('workflows:branch.select-variable')} onChoose={choice => set(name, { type: 'reference', path: choice.path })} />
          : kind === 'boolean' ? <span className={css.toolBoolean}><Switch checked={constant === true} label={name} onChange={on => set(name, { type: 'literal', value: on })} /></span>
            : kind === 'enum' ? <select className={css.input} aria-label={name} value={typeof constant === 'string' ? constant : ''} onChange={event => set(name, event.target.value ? { type: 'literal', value: event.target.value } : undefined)}>
              <option value="">{tx('workflows:tool.unset')}</option>
              {(schema.enum as string[]).map(item => <option key={item} value={item}>{item}</option>)}
            </select>
              : <ConstantInput kind={kind} name={name} value={constant} invalid={required && !value} set={next => set(name, next === undefined ? undefined : { type: 'literal', value: next })} />}
      </div>}
  </div>;
}

/** A number or JSON constant as text, committed once it parses. */
function ConstantInput({ kind, name, value, invalid, set }: { kind: 'integer' | 'number' | 'json'; name: string; value: unknown; invalid: boolean; set: (value: unknown) => void }) {
  const tx = useTranslation(), shown = value === undefined ? '' : kind === 'json' ? JSON.stringify(value, null, 2) : String(value);
  const [text, setText] = useState(shown), [error, setError] = useState(false);
  useEffect(() => { setText(shown); setError(false); }, [shown]);
  const commit = (next: string) => {
    if (!next.trim()) { setError(false); set(undefined); return; }
    let parsed: unknown;
    try { parsed = kind === 'json' ? JSON.parse(next) : Number(next); } catch { setError(true); return; }
    const valid = kind === 'json' || (typeof parsed === 'number' && Number.isFinite(parsed) && (kind === 'number' || Number.isInteger(parsed)));
    setError(!valid);
    if (valid) set(parsed);
  };
  return kind === 'json'
    ? <textarea className={clsx(css.input, css.textarea, css.mono)} aria-label={name} aria-invalid={error || invalid || undefined} placeholder={tx('workflows:tool.json-placeholder')} value={text} spellCheck={false}
      onChange={event => setText(event.target.value)} onBlur={() => commit(text)} />
    : <input className={css.input} inputMode={kind === 'integer' ? 'numeric' : 'decimal'} aria-label={name} aria-invalid={error || invalid || undefined} value={text}
      onChange={event => { setText(event.target.value); commit(event.target.value); }} />;
}

/** Dify's collapsible "Output variables": fixed by the Tool, its declared
 * structured fields or its `text`. */
function ToolOutputs(props: PanelProps & { tool: ToolNode }) {
  const tx = useTranslation(), { node } = props.tool, [open, setOpen] = useState(false);
  const schema = node.result.type === 'json' ? schemaObject(node.result.schema) : undefined, fields = Object.entries(schemaObject(schema?.properties) ?? {});
  return <section className={css.group} aria-label={tx('workflows:prompt.output-variables')}>
    <h3 className={css.groupHeading}>
      <button type="button" className={css.outputToggle} aria-expanded={open} onClick={() => setOpen(value => !value)}>{tx('workflows:prompt.output-variables')}<IconChevronDownOutline14 /></button>
    </h3>
    {open && <dl className={css.outputVars}>
      {node.result.type === 'text'
        ? <div><dt><b>{/* i18n-raw: native output field name */ 'text'}</b><code>{/* i18n-raw: JSON Schema type name */ 'string'}</code></dt><dd>{tx('workflows:tool.output-text')}</dd></div>
        : fields.map(([name, value]) => { const field = schemaObject(value); return <div key={name}>
          <dt><b>{name}</b><code>{String(field?.type ?? '?')}</code></dt>{typeof field?.description === 'string' && <dd>{field.description}</dd>}
        </div>; })}
    </dl>}
  </section>;
}
