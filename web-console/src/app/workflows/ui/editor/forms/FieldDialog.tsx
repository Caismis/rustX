import { useId, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
import { Button } from '../../../../../presentation/primitives/Button';
import { JsonCodeEditor } from '../../../../../presentation/primitives/JsonCodeEditor';
import { Menu } from '../../../../../presentation/primitives/Menu';
import { Modal } from '../../../../../presentation/primitives/Modal';
import { IconCloseOutline16, IconPlusOutline16 } from '../../../../../presentation/primitives/icons';
import { CommandRejected } from '../../../document/commands';
import { updateField, type ArrayEdit, type FieldAddress } from '../../../document/fields';
import { setOrder } from '../../../document/geometry';
import { buildField, FIELD_TYPES, fieldForm, FieldFormError, ITEM_TYPES, MAX_LENGTH_LIMIT, type FieldForm, type FieldType } from '../../../document/inputField';
import { renameSchemaField, schemaObject, schemaTarget, schemaTemplate, type Schema } from '../../../document/schema';
import type { WorkflowDocument } from '../../../document/model';
import { WorkflowIcon } from '../../WorkflowIcon';
import editorCss from '../Editor.module.css';
import css from './InputFields.module.css';

type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;

const TYPE_ICONS: Record<FieldType, string> = {
  string: 'lucide:type', number: 'lucide:hash', integer: 'lucide:binary', boolean: 'lucide:square-check', array: 'lucide:brackets', object: 'lucide:braces',
};
export function FieldTypeIcon({ type }: { type: FieldType }) {
  return <WorkflowIcon icon={TYPE_ICONS[type]} size={14} className={css.typeIcon} />;
}

/** Add or edit one input field, as Dify's variable dialog: a type, a name,
 * per-type limits, and requiredness. An optional field needs a default. */
export function FieldDialog({ document, address, keys, original, schema, required, close, edit }: {
  document: WorkflowDocument; address: FieldAddress; keys: string[]; original?: string; schema?: Schema; required: boolean; close: () => void; edit: Edit;
}) {
  const tx = useTranslation(), id = useId();
  const [form, setForm] = useState(() => fieldForm(original ?? '', schema, required));
  const [error, setError] = useState<{ field: string; message: string }>();
  const [choosing, setChoosing] = useState(false);
  const change = (patch: Partial<FieldForm>) => { setForm(current => ({ ...current, ...patch })); setError(undefined); };
  const fail = (field: string, message: string) => { setError({ field, message }); };
  const problem = (field: string) => error?.field === field ? error.message : undefined;
  const invalid = (field: string) => ({ 'aria-invalid': !!problem(field) || undefined, 'aria-describedby': problem(field) ? `${id}-${field}` : undefined });

  const choose = (type: FieldType) => {
    setChoosing(false);
    if (type === form.type) return;
    // A different type starts from its own template; text that only that
    // type understands is reset.
    change({ type, options: [], maxLength: '', defaultText: '', defaultJson: type === 'array' ? '[]' : '{}', itemType: 'string',
      schemaJson: JSON.stringify(type === 'object' && schema?.type === 'object' ? schema : schemaTemplate('object'), null, 2) });
  };
  const save = () => {
    const key = form.name.trim();
    if (!key) return fail('name', tx('workflows:fields.name-required'));
    if (key !== original && !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) return fail('name', tx('workflows:fields.name-invalid'));
    if (key !== original && keys.includes(key)) return fail('name', tx('workflows:rejection.duplicate_identifier'));
    let property: Schema;
    try { property = buildField({ ...form, name: key }, schema); } catch (failure) {
      if (!(failure instanceof FieldFormError)) throw failure;
      return fail(PROBLEM_FIELD[failure.problem], tx(`workflows:fields.problem.${failure.problem}`));
    }
    const target = schemaTarget(address, document);
    const apply = (current: WorkflowDocument) => {
      let next = original && key !== original ? renameSchemaField(current, address, original, key) : current;
      next = updateField<Schema>(next, address, previous => {
        const fields = (Array.isArray(previous.required) ? previous.required : []).filter(field => field !== key);
        return { ...previous, properties: { ...schemaObject(previous.properties), [key]: property }, required: form.required ? [...fields, key] : fields };
      });
      // A new field goes last, as authors expect from "Add".
      return !original && target ? setOrder(next, target, [...keys, key]) : next;
    };
    try { apply(document); } catch (failure) {
      if (!(failure instanceof CommandRejected)) throw failure;
      return fail('name', tx(`workflows:rejection.${failure.code}`));
    }
    edit(apply);
    close();
  };

  const type = form.type;
  const field = (name: string, label: string, control: (controlId: string) => ReactNode, hint?: string) => <div className={editorCss.field}>
    <label htmlFor={`${id}-${name}-control`} className={editorCss.fieldLabel}>{label}</label>
    {control(`${id}-${name}-control`)}
    {problem(name) ? <small id={`${id}-${name}`} role="alert" className={clsx(editorCss.hint, editorCss.hintError)}>{problem(name)}</small> : hint && <small className={editorCss.hint}>{hint}</small>}
  </div>;

  return <Modal open onClose={close} title={tx(original ? 'workflows:fields.edit-title' : 'workflows:fields.add-title')} closeLabel={tx('workflows:dialog.close')} className={css.dialog}
    footer={<><Button variant="outline" onClick={close}>{tx('workflows:dialog.cancel')}</Button><Button variant="primary" onClick={save}>{tx('workflows:fields.save')}</Button></>}>
    <form className={css.form} onSubmit={event => { event.preventDefault(); save(); }} noValidate>
      {type !== 'raw' && <div className={editorCss.field}>
        <span id={`${id}-type`} className={editorCss.fieldLabel}>{tx('workflows:fields.type-label')}</span>
        <Menu open={choosing} onClose={() => setChoosing(false)} autoFocus selectedId={type} onSelect={value => choose(value as FieldType)} className={css.typeMenu}
          items={FIELD_TYPES.map(option => ({ id: option, icon: <FieldTypeIcon type={option} />, label: <span className={css.typeOption}><span>{tx(`workflows:fields.type.${option}`)}</span><code>{option}</code></span> }))}
          anchor={<button type="button" className={css.typeTrigger} aria-labelledby={`${id}-type ${id}-type-value`} aria-haspopup="menu" aria-expanded={choosing} onClick={() => setChoosing(value => !value)}>
            <FieldTypeIcon type={type} /><span id={`${id}-type-value`}>{tx(`workflows:fields.type.${type}`)}</span><code>{type}</code><span className={css.chevron} aria-hidden>⌄</span>
          </button>} />
      </div>}

      {field('name', tx('workflows:fields.name'), controlId => <input id={controlId} className={clsx(editorCss.input, editorCss.mono)} value={form.name} autoFocus spellCheck={false}
        placeholder={tx('workflows:fields.name-placeholder')} {...invalid('name')} onChange={event => change({ name: event.target.value })} />)}

      {type !== 'raw' && field('description', tx('workflows:fields.description'), controlId => <textarea id={controlId} className={clsx(editorCss.input, css.textarea)} value={form.description}
        placeholder={tx('workflows:fields.description-placeholder')} onChange={event => change({ description: event.target.value })} />)}

      {type === 'string' && <>
        {field('maxLength', tx('workflows:fields.max-length'), controlId => <input id={controlId} className={editorCss.input} type="number" min={1} max={MAX_LENGTH_LIMIT} step={1} inputMode="numeric"
          value={form.maxLength} placeholder={tx('workflows:fields.max-length-placeholder')} {...invalid('maxLength')} onChange={event => change({ maxLength: event.target.value })} />, tx('workflows:fields.max-length-hint', { p0: MAX_LENGTH_LIMIT }))}
        <fieldset className={css.options} aria-describedby={problem('options') ? `${id}-options` : undefined}>
          <legend className={editorCss.fieldLabel}>{tx('workflows:fields.options')}</legend>
          {form.options.map((option, index) => <div key={index} className={css.option}>
            <input className={editorCss.input} value={option} aria-label={tx('workflows:fields.option', { p0: index + 1 })} placeholder={tx('workflows:fields.option-placeholder')}
              onChange={event => change({ options: form.options.map((current, at) => at === index ? event.target.value : current) })} />
            <button type="button" className={editorCss.iconButton} aria-label={tx('workflows:fields.option-remove', { p0: index + 1 })}
              onClick={() => { const options = form.options.filter((_, at) => at !== index); change({ options, defaultText: options.includes(form.defaultText) ? form.defaultText : '' }); }}><IconCloseOutline16 /></button>
          </div>)}
          <button type="button" className={css.addOption} onClick={() => change({ options: [...form.options, ''] })}><IconPlusOutline16 />{tx('workflows:fields.option-add')}</button>
          {problem('options') ? <small id={`${id}-options`} role="alert" className={clsx(editorCss.hint, editorCss.hintError)}>{problem('options')}</small> : <small className={editorCss.hint}>{tx('workflows:fields.options-hint')}</small>}
        </fieldset>
      </>}

      {type === 'array' && field('items', tx('workflows:fields.items'), controlId => <select id={controlId} className={editorCss.input} value={form.itemType} onChange={event => change({ itemType: event.target.value })}>
        {ITEM_TYPES.map(item => <option key={item} value={item}>{item ? `${tx(`workflows:fields.type.${item as FieldType}`)} · ${item}` : tx('workflows:fields.items-any')}</option>)}
      </select>)}

      {(type === 'object' || type === 'raw') && <div className={css.code}>
        <JsonCodeEditor label={tx(type === 'raw' ? 'workflows:fields.json' : 'workflows:fields.object-schema')} value={form.schemaJson} allowEmpty={false} minHeight={160}
          error={problem('schema')} onChange={schemaJson => change({ schemaJson })} />
        <small className={editorCss.hint}>{tx('workflows:fields.json-hint')}</small>
      </div>}

      {type !== 'raw' && <>
        <fieldset className={css.requirement}>
          <legend className={editorCss.fieldLabel}>{tx('workflows:fields.requirement')}</legend>
          {([true, false] as const).map(value => <label key={String(value)} className={clsx(css.requirementOption, form.required === value && css.requirementActive)}>
            <input type="radio" name={`${id}-required`} checked={form.required === value} onChange={() => change({ required: value })} />
            <span><b>{tx(value ? 'workflows:schema.required' : 'workflows:fields.optional')}</b><small>{tx(value ? 'workflows:fields.required-hint' : 'workflows:fields.optional-hint')}</small></span>
          </label>)}
        </fieldset>
        {!form.required && <DefaultValue id={id} form={form} change={change} problem={problem('default')} />}
      </>}
      <button type="submit" hidden />
    </form>
  </Modal>;
}

/** The value an optional field takes when the caller leaves it out. */
function DefaultValue({ id, form, change, problem }: { id: string; form: FieldForm; change: (patch: Partial<FieldForm>) => void; problem?: string }) {
  const tx = useTranslation(), label = tx('workflows:fields.default');
  const control = `${id}-default-control`;
  const described = { 'aria-invalid': !!problem || undefined, 'aria-describedby': problem ? `${id}-default` : undefined };
  const hint = problem ? <small id={`${id}-default`} role="alert" className={clsx(editorCss.hint, editorCss.hintError)}>{problem}</small> : <small className={editorCss.hint}>{tx('workflows:fields.default-hint')}</small>;
  if (form.type === 'array' || form.type === 'object') return <div className={css.code}>
    <JsonCodeEditor label={label} value={form.defaultJson} allowEmpty={false} minHeight={96} error={problem} onChange={defaultJson => change({ defaultJson })} />
  </div>;
  const options = form.options.map(option => option.trim()).filter(Boolean);
  return <div className={editorCss.field}>
    <label htmlFor={control} className={editorCss.fieldLabel}>{label}</label>
    {form.type === 'boolean'
      ? <select id={control} className={editorCss.input} value={String(form.defaultBool)} onChange={event => change({ defaultBool: event.target.value === 'true' })}>
        <option value="true">true</option><option value="false">false</option>
      </select>
      : form.type === 'string' && options.length
        ? <select id={control} className={editorCss.input} value={options.includes(form.defaultText) ? form.defaultText : ''} {...described} onChange={event => change({ defaultText: event.target.value })}>
          <option value="" disabled>{tx('workflows:fields.default-choose')}</option>
          {[...new Set(options)].map(option => <option key={option} value={option}>{option}</option>)}
        </select>
        : <input id={control} className={clsx(editorCss.input, form.type !== 'string' && editorCss.mono)} value={form.defaultText} {...described}
          type={form.type === 'string' ? 'text' : 'number'} step={form.type === 'integer' ? 1 : 'any'} inputMode={form.type === 'string' ? undefined : form.type === 'integer' ? 'numeric' : 'decimal'}
          maxLength={form.type === 'string' && Number(form.maxLength) > 0 ? Number(form.maxLength) : undefined}
          placeholder={tx(form.type === 'string' ? 'workflows:fields.default-text-placeholder' : 'workflows:fields.default-number-placeholder')} onChange={event => change({ defaultText: event.target.value })} />}
    {hint}
  </div>;
}

const PROBLEM_FIELD: Record<FieldFormError['problem'], string> = {
  'json-invalid': 'schema', 'object-schema': 'schema', 'max-length-range': 'maxLength', 'options-empty': 'options',
  'default-too-long': 'default', 'default-option': 'default', 'default-number': 'default', 'default-integer': 'default', 'default-json': 'default',
};
