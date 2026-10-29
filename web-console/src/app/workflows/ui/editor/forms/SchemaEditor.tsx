import { useContext, useState, type ReactNode } from 'react';
import { useSelector } from '@xstate/react';
import { DraftOwner } from './drafts';
import { useTranslation } from '../../../../../locale/react';
import { encodeField, updateField, within, type ArrayEdit, type FieldAddress } from '../../../document/fields';
import { setOrder } from '../../../document/geometry';
import { orderedKeys } from '../../../document/presentation';
import { patchSchema, renameSchemaField, schemaConsumers, schemaObject, schemaPropertyAddress, schemaTarget, schemaTemplate, schemaTypes, type Schema } from '../../../document/schema';
import type { WorkflowDocument } from '../../../document/model';
import { AddKey } from './ValueEditor';
import { JsonField, KeyField, SelectField, TextField } from './fields';
import css from '../Editor.module.css';

type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string, array?: ArrayEdit) => void;

/** Visual controls and advanced JSON edit the same native schema. Merely
 * opening either representation never normalizes or replaces the schema. */
export function SchemaEditor({ label, value, address, edit, depth = 0 }: { label: string; value: unknown; address: FieldAddress; edit: Edit; depth?: number }) {
  const tx = useTranslation();
  const [advanced, setAdvanced] = useState(false);
  const owner = useContext(DraftOwner)!;
  const pending = useSelector(owner.editor, state => !!state.context.drafts[encodeField(address)]);
  const showJson = advanced || pending;
  const schema = schemaObject(value), type = typeof schema?.type === 'string' ? schema.type : '';
  const properties = schemaObject(schema?.properties), target = schemaTarget(address);
  const set = (patch: Schema, session?: string) => edit(document => patchSchema(document, address, patch), session);
  const keys = properties ? Object.keys(properties) : [];
  return <section className={css.schemaEditor} data-schema-label={label}>
    <h3 className={css.groupHeading}><span>{label}</span><button type="button" className={css.linkButton} aria-expanded={showJson} onClick={() => setAdvanced(value => !value)}>{tx(showJson ? 'workflows:schema.visual' : 'workflows:schema.json')}</button></h3>
    <SchemaConsumers address={address} />
    {showJson || !schema || !schemaTypes.includes(type as never) || depth >= 16
      ? <JsonField label={label} hint={tx('workflows:inspector.schema-hint')} value={value} rows={8} address={address} />
      : <>
        <SelectField label={tx('workflows:schema.type')} value={type} options={schemaTypes.map(value => ({ value, label: value }))}
          onChange={type => { if (type !== schema.type) edit(document => updateField<Schema>(document, address, current => ({ ...schemaTemplate(type), ...(current.description !== undefined ? { description: current.description } : {}) }))); }} />
        <TextField label={tx('workflows:schema.description')} value={typeof schema.description === 'string' ? schema.description : ''} field={`${encodeField(address)}:description`}
          onChange={(description, session) => set({ description }, session)} />
        {type === 'object' && <>
          <SchemaProperties address={address} properties={properties ?? {}} required={Array.isArray(schema.required) ? schema.required : []} target={target} edit={edit} depth={depth} />
          <label className={css.check}><input type="checkbox" checked={schema.additionalProperties !== false} onChange={event => set({ additionalProperties: event.target.checked })} />{tx('workflows:schema.additional')}</label>
        </>}
        {type === 'array' && <>
          <label className={css.check}><input type="checkbox" checked={Object.hasOwn(schema, 'items')} onChange={event => {
            const enabled = event.target.checked;
            edit(document => updateField<Schema>(document, address, current => {
              const { items: _items, ...rest } = current;
              return enabled ? { ...current, items: schemaTemplate('string') } : rest;
            }));
          }} />{tx('workflows:schema.items-enabled')}</label>
          {Object.hasOwn(schema, 'items')
            ? <SchemaEditor label={tx('workflows:schema.items')} value={schema.items} address={within(address, ['items'])} edit={edit} depth={depth + 1} />
            : <p className={css.hint}>{tx('workflows:schema.items-unconstrained')}</p>}
        </>}
        {!['integer', 'number'].includes(type) && <details className={css.advanced}><summary>{tx('workflows:schema.constraints')}</summary>
          {(['enum', 'const'] as const).map(key => <div key={key}>
            <label className={css.check}><input type="checkbox" checked={Object.hasOwn(schema, key)} onChange={event => {
              const enabled = event.target.checked;
              edit(document => updateField<Schema>(document, address, current => {
                const { [key]: _old, ...rest } = current;
                const literal = type === 'string' ? '' : type === 'boolean' ? false : type === 'array' ? [] : type === 'object' ? {} : null;
                return enabled ? { ...current, [key]: key === 'enum' ? [literal] : literal } : rest;
              }));
            }} />{key}</label>
            {Object.hasOwn(schema, key) && <JsonField label={key} value={schema[key]} address={within(address, [key])} rows={3} />}
          </div>)}
        </details>}
        <p className={css.hint}>{tx('workflows:schema.type-change')}</p>
      </>}
    {!advanced && properties && keys.length > 256 && <p role="alert" className={css.hintError}>{tx('workflows:rejection.limit_exceeded')}</p>}
  </section>;
}

function SchemaProperties({ address, properties, required, target, edit, depth }: { address: FieldAddress; properties: Schema; required: unknown[]; target: ReturnType<typeof schemaTarget>; edit: Edit; depth: number }) {
  const tx = useTranslation();
  // Obtain display order from the current document at the owner, rather than
  // assigning meaning to property-map insertion order.
  return <SchemaOrder target={target} keys={Object.keys(properties)}>{keys => <>
    {keys.map((key, index) => <details className={css.schemaProperty} key={key} open={keys.length === 1}>
      <summary><span>{key}</span><code>{String(schemaObject(properties[key])?.type ?? '?')}</code>{required.includes(key) && <span aria-label={tx('workflows:schema.required')}>*</span>}</summary>
      <KeyField label={tx('workflows:schema.name')} value={key} address={schemaPropertyAddress(address, key)}
        rename={(document, from, to) => renameSchemaField(document, address, from, to)} />
      <label className={css.check}><input type="checkbox" checked={required.includes(key)} onChange={event => {
        const checked = event.target.checked;
        edit(document => updateField<Schema>(document, address, current => ({ ...current, required: checked ? [...new Set([...(Array.isArray(current.required) ? current.required : []), key])] : (Array.isArray(current.required) ? current.required : []).filter(field => field !== key) })));
      }} />{tx('workflows:schema.required')}</label>
      <SchemaEditor label={key} value={properties[key]} address={within(address, ['properties', { key }])} edit={edit} depth={depth + 1} />
      <div className={css.row}>
        {target && <><button type="button" className={css.smallButton} disabled={index === 0} aria-label={tx('workflows:schema.up', { p0: key })} onClick={() => edit(document => { const fields = orderedKeys(document, target, keys); [fields[index - 1], fields[index]] = [fields[index], fields[index - 1]]; return setOrder(document, target, fields); })}>↑</button>
          <button type="button" className={css.smallButton} disabled={index === keys.length - 1} aria-label={tx('workflows:schema.down', { p0: key })} onClick={() => edit(document => { const fields = orderedKeys(document, target, keys); [fields[index + 1], fields[index]] = [fields[index], fields[index + 1]]; return setOrder(document, target, fields); })}>↓</button></>}
        <button type="button" className={css.smallButton} onClick={() => edit(document => updateField<Schema>(document, address, current => ({ ...current,
          properties: Object.fromEntries(Object.entries(schemaObject(current.properties) ?? {}).filter(([name]) => name !== key)),
          ...(Array.isArray(current.required) ? { required: current.required.filter(name => name !== key) } : {}),
        })))}>{tx('workflows:inspector.remove', { p0: key })}</button>
      </div>
    </details>)}
    <AddKey existing={keys} address={address} add={(document, key) => updateField<Schema>(document, address, current => ({ ...current, properties: { ...schemaObject(current.properties), [key]: { type: 'string' } } }))} />
    <p className={css.hint}>{tx('workflows:schema.affected')}</p>
  </>}</SchemaOrder>;
}

function SchemaOrder({ target, keys, children }: { target: ReturnType<typeof schemaTarget>; keys: string[]; children: (keys: string[]) => ReactNode }) {
  const owner = useContext(DraftOwner)!;
  const document = useSelector(owner.editor, state => state.context.document);
  return children(document && target ? orderedKeys(document, target, keys) : keys.sort());
}

function SchemaConsumers({ address }: { address: FieldAddress }) {
  const tx = useTranslation(), owner = useContext(DraftOwner)!;
  const document = useSelector(owner.editor, state => state.context.document);
  const uses = document ? schemaConsumers(document, address) : [];
  return uses.length ? <p className={css.hint}>{tx('workflows:schema.consumers', { p0: uses.map(use => [...use.block.map(step => `${step.node}/body`), use.node].join(' › ')).join(', ') })}</p> : null;
}
