import { useState } from 'react';
import { Menu } from '@base-ui/react/menu';
import { useTranslation } from '../../../../../../locale/react';
import { Switch } from '../../../../../../presentation/primitives/Switch';
import { IconChevronDownOutline14, IconChevronRightOutline14, IconEditOutline16, IconTrashOutline16 } from '../../../../../../presentation/primitives/icons';
import { IconPlus } from '../../../icons';
import { encodeField, within, type FieldAddress } from '../../../../document/fields';
import type { WorkflowDocument } from '../../../../document/model';
import { orderedKeys } from '../../../../document/presentation';
import { schemaObject, schemaTarget, schemaTemplate, schemaTypes, type Schema } from '../../../../document/schema';
import { putProperty, removeProperty, schemaAt, type SchemaEdit } from './schema';
import css from './StructuredOutput.module.css';

interface Editing { parent: FieldAddress; name?: string }
export const fieldType = (schema: Schema) => schema.type === 'array' ? `array[${schemaObject(schema.items)?.type ?? 'string'}]` : String(schema.type ?? 'string');
const types = [...schemaTypes.filter(type => type !== 'array'), ...schemaTypes.filter(type => type !== 'array').map(type => `array[${type}]`)];

export function VisualEditor({ document, address, root, change, editing, setEditing }: {
  document: WorkflowDocument; address: FieldAddress; root: string; change: (apply: SchemaEdit) => boolean; editing: Editing | undefined; setEditing: (editing: Editing | undefined) => void;
}) {
  return <div className={css.visual}>
    <SchemaNode name={root} document={document} address={address} depth={0} change={change} editing={editing} setEditing={setEditing} />
  </div>;
}

interface NodeProps {
  name: string; document: WorkflowDocument; address: FieldAddress; depth: number; parent?: FieldAddress; required?: boolean;
  change: (apply: SchemaEdit) => boolean; editing: Editing | undefined; setEditing: (editing: Editing | undefined) => void;
}

function SchemaNode(props: NodeProps) {
  const { name, document, address, depth, parent, required, change, editing, setEditing } = props;
  const tx = useTranslation(), [expanded, setExpanded] = useState(true);
  const schema = schemaAt(document, address), type = fieldType(schema);
  const objectAddress = schema.type === 'object' ? address : schema.type === 'array' && schemaObject(schema.items)?.type === 'object' ? within(address, ['items']) : undefined;
  const object = objectAddress ? schemaAt(document, objectAddress) : undefined;
  const properties = schemaObject(object?.properties) ?? {}, target = objectAddress && schemaTarget(objectAddress);
  const keys = target ? orderedKeys(document, target, Object.keys(properties)) : Object.keys(properties);
  const active = !!parent && editing?.name === name && encodeField(editing.parent) === encodeField(parent);
  const adding = objectAddress && editing?.name === undefined && editing && encodeField(editing.parent) === encodeField(objectAddress);
  return <div className={css.node} data-schema-field={name}>
    {active ? <FieldCard key={`${encodeField(parent!)}:${name}`} document={document} parent={parent!} name={name} onCancel={() => setEditing(undefined)} onConfirm={apply => { if (change(apply)) setEditing(undefined); }} />
      : <div className={css.nodeRow}>
        {depth > 0 && objectAddress && <button type="button" className={css.expand} aria-label={tx('workflows:structured.expand', { p0: name })} aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>{expanded ? <IconChevronDownOutline14 /> : <IconChevronRightOutline14 />}</button>}
        <div className={css.nodeText}><div className={css.nodeName}><b>{name}</b><code>{type}</code>{required && <small>{tx('workflows:schema.required')}</small>}</div>
          {typeof schema.description === 'string' && schema.description && <p>{schema.description}</p>}
        </div>
        <div className={css.nodeActions}>
          {objectAddress && depth < 16 && <button type="button" disabled={!!editing} aria-label={tx('workflows:structured.add-child', { p0: name })} onClick={() => { setExpanded(true); setEditing({ parent: objectAddress }); }}><IconPlus size={14} /></button>}
          {parent && <><button type="button" disabled={!!editing} aria-label={tx('workflows:structured.edit-field', { p0: name })} onClick={() => setEditing({ parent, name })}><IconEditOutline16 /></button>
            <button type="button" disabled={!!editing} aria-label={tx('workflows:structured.remove-field', { p0: name })} onClick={() => change(removeProperty(parent, name))}><IconTrashOutline16 /></button></>}
        </div>
      </div>}
    {expanded && objectAddress && depth < 16 && <div className={css.children}>
      {keys.map(key => <SchemaNode key={key} {...props} name={key} parent={objectAddress} address={within(objectAddress, ['properties', { key }])} depth={depth + 1} required={Array.isArray(object?.required) && object.required.includes(key)} />)}
      {adding && <FieldCard key={encodeField(objectAddress)} document={document} parent={objectAddress} onCancel={() => setEditing(undefined)} onConfirm={apply => { if (change(apply)) setEditing(undefined); }} />}
      {!depth && !editing && <button type="button" className={css.addField} onClick={() => setEditing({ parent: objectAddress })}><IconPlus size={14} />{tx('workflows:structured.add-field')}</button>}
    </div>}
  </div>;
}

function FieldCard({ document, parent, name, onCancel, onConfirm }: {
  document: WorkflowDocument; parent: FieldAddress; name?: string; onCancel: () => void; onConfirm: (apply: SchemaEdit) => void;
}) {
  const tx = useTranslation(), object = schemaAt(document, parent);
  const original = name !== undefined ? schemaObject(schemaObject(object.properties)?.[name]) ?? schemaTemplate('string') : schemaTemplate('string');
  const [field, setField] = useState(name ?? ''), [type, setType] = useState(fieldType(original));
  const [required, setRequired] = useState(name === undefined || Array.isArray(object.required) && object.required.includes(name));
  const [description, setDescription] = useState(typeof original.description === 'string' ? original.description : '');
  const originalEnums = Array.isArray(original.enum) ? original.enum.join(', ') : '';
  const [enums, setEnums] = useState(originalEnums);
  const [error, setError] = useState('');
  const confirm = () => {
    const key = field.trim();
    if (!key || new TextEncoder().encode(key).length > 64) { setError(tx('workflows:structured.invalid-name')); return; }
    if (key !== name && Object.hasOwn(schemaObject(object.properties) ?? {}, key)) { setError(tx('workflows:structured.duplicate-name')); return; }
    let schema = type === fieldType(original) ? { ...original } : type.startsWith('array[') ? { type: 'array', items: schemaTemplate(type.slice(6, -1)) } : schemaTemplate(type);
    if (description) schema.description = description; else delete schema.description;
    if (type === 'string' && (type !== fieldType(original) || enums !== originalEnums)) {
      const values = enums.split(',').map(value => value.trim()).filter(Boolean);
      if (values.length) schema.enum = [...new Set(values)]; else delete schema.enum;
    }
    onConfirm(putProperty(parent, name, key, schema, required));
  };
  return <form className={css.fieldCard} aria-label={tx('workflows:structured.field-editor')} onSubmit={event => { event.preventDefault(); confirm(); }} onKeyDown={event => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); confirm(); }
    if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onCancel(); }
  }}>
    <div className={css.fieldTop}>
      <input autoFocus className={css.fieldName} aria-label={tx('workflows:schema.name')} placeholder={tx('workflows:schema.name')} value={field} onChange={event => { setField(event.target.value); setError(''); }} />
      <TypeSelector value={type} onChange={setType} />
      <div className={css.fieldActions}><span className={css.required}>{tx('workflows:schema.required')}<Switch className={css.requiredSwitch} label={tx('workflows:schema.required')} checked={required} onChange={setRequired} /></span>
        <span className={css.separator} /><button type="button" onClick={onCancel}>{tx('workflows:dialog.cancel')}</button>
        <button type="submit" className={css.confirm} disabled={!field.trim()}>{tx('workflows:structured.confirm')}<kbd>{/* i18n-raw: keyboard shortcut */ 'Ctrl ↵'}</kbd></button>
      </div>
    </div>
    <input className={css.description} aria-label={tx('workflows:schema.description')} placeholder={tx('workflows:panel.description-placeholder')} value={description} onChange={event => setDescription(event.target.value)} />
    {type === 'string' && <div className={css.validations}><div className={css.validationTitle}>{tx('workflows:structured.string-validations')}<span /></div>
      <label>{tx('workflows:structured.enum')}<textarea aria-label={tx('workflows:structured.enum')} placeholder={tx('workflows:structured.enum-placeholder')} rows={2} value={enums} onChange={event => setEnums(event.target.value)} /></label>
    </div>}
    {error && <p role="alert" className={css.error}>{error}</p>}
  </form>;
}

function TypeSelector({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const tx = useTranslation();
  return <Menu.Root modal={false}><Menu.Trigger className={css.typeTrigger} aria-label={tx('workflows:schema.type')}>{value}<IconChevronDownOutline14 /></Menu.Trigger>
    <Menu.Portal><Menu.Positioner className={css.menuPositioner} sideOffset={4} align="start"><Menu.Popup className={css.typeMenu}>
      {(types.includes(value) ? types : [...types, value]).map(type => <Menu.Item key={type} className={css.typeItem} onClick={() => onChange(type)}>{type}</Menu.Item>)}
    </Menu.Popup></Menu.Positioner></Menu.Portal>
  </Menu.Root>;
}
