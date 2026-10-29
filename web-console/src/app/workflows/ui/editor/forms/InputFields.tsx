import { useContext, useId, useState, type DragEvent } from 'react';
import { useSelector } from '@xstate/react';
import clsx from 'clsx';
import { DraftOwner } from './drafts';
import { useTranslation } from '../../../../../locale/react';
import { Button } from '../../../../../presentation/primitives/Button';
import { Modal } from '../../../../../presentation/primitives/Modal';
import { Switch } from '../../../../../presentation/primitives/Switch';
import { IconEditOutline16, IconPlusOutline16, IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { CommandRejected } from '../../../document/commands';
import { encodeField, updateField, type ArrayEdit, type FieldAddress } from '../../../document/fields';
import { setOrder } from '../../../document/geometry';
import { orderedKeys } from '../../../document/presentation';
import { renameSchemaField, schemaObject, schemaTarget, schemaTemplate, schemaTypes, type Schema } from '../../../document/schema';
import type { WorkflowDocument } from '../../../document/model';
import { readJson } from '../../../../../../shared/json-numbers';
import { JsonField } from './fields';
import editorCss from '../Editor.module.css';
import css from './InputFields.module.css';

type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;

/** Dify's Start-node field kinds, each one shape of a JSON Schema property. */
const KINDS = ['text', 'select', 'number', 'integer', 'boolean', 'array', 'object'] as const;
type Kind = typeof KINDS[number];
const KIND_TYPE: Record<Kind, string> = { text: 'string', select: 'string', number: 'number', integer: 'integer', boolean: 'boolean', array: 'array', object: 'object' };

/** The kind a property is edited as; anything else is edited as JSON. */
function kindOf(schema: Schema | undefined): Kind | undefined {
  const type = schema?.type;
  if (type === 'string') return Array.isArray(schema!.enum) ? 'select' : 'text';
  return KINDS.find(kind => kind === type);
}

/** Workflow input fields as a Dify-style list over the native object schema:
 * one row per property, a dialog to add or edit one, drag to reorder.
 * Advanced JSON stays available for every schema. */
export function InputFields({ label, value, address, edit }: { label: string; value: unknown; address: FieldAddress; edit: Edit }) {
  const tx = useTranslation();
  const owner = useContext(DraftOwner)!;
  const document = useSelector(owner.editor, state => state.context.document);
  const pending = useSelector(owner.editor, state => !!state.context.drafts[encodeField(address)]);
  const [advanced, setAdvanced] = useState(false);
  const [editing, setEditing] = useState<string | null>();
  const [dragging, setDragging] = useState<string>();
  const schema = schemaObject(value), properties = schemaObject(schema?.properties);
  const target = schemaTarget(address);
  const visual = !advanced && !pending && schema?.type === 'object' && !!properties;
  const keys = properties ? (document && target ? orderedKeys(document, target, Object.keys(properties)) : Object.keys(properties).sort()) : [];
  const required = Array.isArray(schema?.required) ? schema.required : [];
  const move = (from: number, to: number) => {
    if (!target || to < 0 || to >= keys.length || from === to) return;
    const fields = [...keys]; fields.splice(to, 0, ...fields.splice(from, 1));
    edit(current => setOrder(current, target, fields));
  };
  const remove = (key: string) => edit(current => updateField<Schema>(current, address, previous => ({ ...previous,
    properties: Object.fromEntries(Object.entries(schemaObject(previous.properties) ?? {}).filter(([name]) => name !== key)),
    ...(Array.isArray(previous.required) ? { required: previous.required.filter(name => name !== key) } : {}),
  })));
  const drop = (event: DragEvent, key: string) => { event.preventDefault(); if (dragging) move(keys.indexOf(dragging), keys.indexOf(key)); setDragging(undefined); };

  return <section className={editorCss.schemaEditor} data-schema-label={label}>
    <h3 className={editorCss.groupHeading}><span>{label}</span>
      <span className={css.headActions}>
        <button type="button" className={editorCss.linkButton} aria-expanded={!visual} onClick={() => setAdvanced(value => !value)}>{tx(visual ? 'workflows:schema.json' : 'workflows:schema.visual')}</button>
        {visual && <button type="button" className={editorCss.iconButton} aria-label={tx('workflows:fields.add')} title={tx('workflows:fields.add')} onClick={() => setEditing(null)}><IconPlusOutline16 /></button>}
      </span>
    </h3>
    {!visual
      ? <JsonField label={label} hint={tx('workflows:inspector.schema-hint')} value={value} rows={10} address={address} />
      : keys.length === 0
        ? <button type="button" className={css.empty} onClick={() => setEditing(null)}>{tx('workflows:fields.empty')}</button>
        : <ul className={css.list} aria-label={label}>{keys.map((key, index) => {
          const field = schemaObject(properties![key]), kind = kindOf(field);
          const description = typeof field?.description === 'string' ? field.description : '';
          return <li key={key} className={clsx(css.row, dragging === key && css.dragging)} draggable={keys.length > 1 && !!target}
            onDragStart={event => { event.dataTransfer.effectAllowed = 'move'; setDragging(key); }} onDragEnd={() => setDragging(undefined)}
            onDragOver={event => { if (dragging) event.preventDefault(); }} onDrop={event => drop(event, key)}>
            <button type="button" className={css.rowMain} aria-label={tx('workflows:fields.edit', { p0: key })} aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown" onClick={() => setEditing(key)}
              onKeyDown={event => { if (event.altKey && (event.key === 'ArrowUp' || event.key === 'ArrowDown')) { event.preventDefault(); move(index, index + (event.key === 'ArrowUp' ? -1 : 1)); } }}>
              <span className={css.variable} aria-hidden>{'{x}'}</span>
              <span className={css.name}>{key}</span>
              {description && <><span className={css.dot} aria-hidden>·</span><span className={css.detail} title={description}>{description}</span></>}
            </button>
            {required.includes(key) && <span className={css.required}>{tx('workflows:schema.required')}</span>}
            <span className={css.type}>{kind ? tx(`workflows:fields.kind.${kind}`) : String(field?.type ?? 'JSON')}</span>
            <span className={css.rowActions}>
              <button type="button" className={editorCss.iconButton} aria-label={tx('workflows:fields.edit', { p0: key })} tabIndex={-1} onClick={() => setEditing(key)}><IconEditOutline16 /></button>
              <button type="button" className={editorCss.iconButton} aria-label={tx('workflows:inspector.remove', { p0: key })} onClick={() => remove(key)}><IconTrashOutline16 /></button>
            </span>
          </li>;
        })}</ul>}
    {editing !== undefined && document && <FieldDialog document={document} address={address} keys={keys} original={editing ?? undefined}
      schema={editing ? schemaObject(properties?.[editing]) : undefined} required={!!editing && required.includes(editing)}
      close={() => setEditing(undefined)} edit={edit} />}
  </section>;
}

function FieldDialog({ document, address, keys, original, schema, required: wasRequired, close, edit }: {
  document: WorkflowDocument; address: FieldAddress; keys: string[]; original?: string; schema?: Schema; required: boolean; close: () => void; edit: Edit;
}) {
  const tx = useTranslation(), id = useId();
  const initial = original ? kindOf(schema) : 'text';
  const [kind, setKind] = useState<Kind | undefined>(initial);
  const [name, setName] = useState(original ?? '');
  const [description, setDescription] = useState(typeof schema?.description === 'string' ? schema.description : '');
  const [required, setRequired] = useState(original ? wasRequired : true);
  const [options, setOptions] = useState(Array.isArray(schema?.enum) ? schema.enum.map(String).join('\n') : '');
  const items = schemaObject(schema?.items);
  const [itemType, setItemType] = useState(typeof items?.type === 'string' ? items.type : schema && !items ? '' : 'string');
  const [json, setJson] = useState(JSON.stringify(schema ?? schemaTemplate('object'), null, 2));
  const [error, setError] = useState<string>();
  const structured = kind !== undefined && kind !== 'object';

  const build = (): Schema => {
    if (!structured) {
      const reading = readJson(json), parsed = reading.ok ? schemaObject(reading.value) : undefined;
      if (!parsed) throw new Error(tx('workflows:inspector.json-invalid'));
      return parsed;
    }
    // The same kind keeps its other keywords; a new kind starts clean.
    const { description: _d, enum: _e, items: _i, ...rest } = kind === initial && schema ? schema : schemaTemplate(KIND_TYPE[kind]);
    const next: Schema = { ...rest, type: KIND_TYPE[kind] };
    if (description.trim()) next.description = description.trim();
    if (kind === 'select') {
      const choices = [...new Set(options.split('\n').map(option => option.trim()).filter(Boolean))];
      if (!choices.length) throw new Error(tx('workflows:fields.options-required'));
      next.enum = choices;
    }
    if (kind === 'array' && itemType) next.items = kind === initial && items?.type === itemType ? items : schemaTemplate(itemType);
    return next;
  };
  const save = () => {
    const key = name.trim();
    try {
      if (!key) throw new Error(tx('workflows:fields.name-required'));
      if (key !== original && !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) throw new Error(tx('workflows:fields.name-invalid'));
      if (key !== original && keys.includes(key)) throw new Error(tx('workflows:rejection.duplicate_identifier'));
      const property = build(), target = schemaTarget(address);
      const apply = (current: WorkflowDocument) => {
        let next = original && key !== original ? renameSchemaField(current, address, original, key) : current;
        next = updateField<Schema>(next, address, previous => {
          const fields = (Array.isArray(previous.required) ? previous.required : []).filter(field => field !== key);
          return { ...previous, properties: { ...schemaObject(previous.properties), [key]: property }, required: required ? [...fields, key] : fields };
        });
        // A new field goes last, as authors expect from "Add".
        return !original && target ? setOrder(next, target, [...keys, key]) : next;
      };
      apply(document);
      edit(apply);
      close();
    } catch (failure) {
      setError(failure instanceof CommandRejected ? tx(`workflows:rejection.${failure.code}`) : (failure as Error).message);
    }
  };

  return <Modal open onClose={close} title={tx(original ? 'workflows:fields.edit-title' : 'workflows:fields.add-title')} closeLabel={tx('workflows:dialog.close')} className={css.dialog}
    footer={<><Button variant="outline" onClick={close}>{tx('workflows:dialog.cancel')}</Button><Button variant="primary" onClick={save}>{tx('workflows:fields.save')}</Button></>}>
    <form className={css.form} onSubmit={event => { event.preventDefault(); save(); }} noValidate>
      <fieldset className={css.kinds}><legend className={editorCss.fieldLabel}>{tx('workflows:fields.type')}</legend>
        {KINDS.map(option => <button type="button" key={option} className={clsx(css.kind, kind === option && css.kindActive)} aria-pressed={kind === option} onClick={() => { setKind(option); setError(undefined); if (option === 'object' && initial !== 'object') setJson(JSON.stringify(schemaTemplate('object'), null, 2)); }}>
          <span className={css.kindGlyph} aria-hidden>{GLYPH[option]}</span>{tx(`workflows:fields.kind.${option}`)}
        </button>)}
      </fieldset>
      <label className={editorCss.field}><span className={editorCss.fieldLabel}>{tx('workflows:fields.name')}</span>
        <input className={clsx(editorCss.input, editorCss.mono)} value={name} autoFocus spellCheck={false} placeholder={tx('workflows:fields.name-placeholder')} aria-invalid={!!error || undefined} aria-describedby={error ? `${id}-error` : undefined}
          onChange={event => { setName(event.target.value); setError(undefined); }} />
      </label>
      {structured && <label className={editorCss.field}><span className={editorCss.fieldLabel}>{tx('workflows:fields.description')}</span>
          <textarea className={clsx(editorCss.input, css.textarea)} value={description} placeholder={tx('workflows:fields.description-placeholder')} onChange={event => setDescription(event.target.value)} />
      </label>}
      {kind === 'select' && <label className={editorCss.field}><span className={editorCss.fieldLabel}>{tx('workflows:fields.options')}</span>
        <textarea className={clsx(editorCss.input, css.textarea)} value={options} placeholder={tx('workflows:fields.options-placeholder')} onChange={event => { setOptions(event.target.value); setError(undefined); }} />
      </label>}
      {kind === 'array' && <label className={editorCss.field}><span className={editorCss.fieldLabel}>{tx('workflows:fields.items')}</span>
        <select className={editorCss.input} value={itemType} onChange={event => setItemType(event.target.value)}>
          <option value="">{tx('workflows:fields.items-any')}</option>
          {schemaTypes.map(type => <option key={type} value={type}>{type}</option>)}
        </select>
      </label>}
      {!structured && <label className={editorCss.field}><span className={editorCss.fieldLabel}>{tx('workflows:fields.json')}</span>
        <textarea className={clsx(editorCss.input, css.textarea, css.json, editorCss.mono)} value={json} spellCheck={false} onChange={event => { setJson(event.target.value); setError(undefined); }} />
        <small className={editorCss.hint}>{tx('workflows:fields.json-hint')}</small>
      </label>}
      <div className={css.switchRow}><span>{tx('workflows:schema.required')}</span><Switch checked={required} onChange={setRequired} label={tx('workflows:schema.required')} /></div>
      {error && <p id={`${id}-error`} role="alert" className={clsx(editorCss.hint, editorCss.hintError)}>{error}</p>}
      <button type="submit" hidden />
    </form>
  </Modal>;
}

const GLYPH: Record<Kind, string> = { text: 'T', select: '☰', number: '#', integer: '1', boolean: '✓', array: '[ ]', object: '{ }' };
