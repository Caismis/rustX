import { useContext, useState, type DragEvent } from 'react';
import { useSelector } from '@xstate/react';
import clsx from 'clsx';
import { DraftOwner } from './drafts';
import { useTranslation } from '../../../../../locale/react';
import { IconEditOutline16, IconPlusOutline16, IconTrashOutline16 } from '../../../../../presentation/primitives/icons';
import { encodeField, updateField, type ArrayEdit, type FieldAddress } from '../../../document/fields';
import { setOrder } from '../../../document/geometry';
import { orderedKeys } from '../../../document/presentation';
import { schemaObject, schemaTarget, type Schema } from '../../../document/schema';
import { FIELD_TYPES } from '../../../document/inputField';
import type { WorkflowDocument } from '../../../document/model';
import { JsonField } from './fields';
import { FieldDialog, FieldTypeIcon } from './FieldDialog';
import editorCss from '../Editor.module.css';
import css from './InputFields.module.css';

type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;

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
          const field = schemaObject(properties![key]), type = FIELD_TYPES.find(type => type === field?.type);
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
            <span className={css.required}>{tx(required.includes(key) ? 'workflows:schema.required' : 'workflows:fields.optional')}</span>
            <span className={css.type}>{type ? <><FieldTypeIcon type={type} />{tx(`workflows:fields.type.${type}`)}</> : String(field?.type ?? 'JSON')}</span>
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
