import { useState } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
import { encodeField, fieldValue, setField, updateField, within, type FieldAddress } from '../../../document/fields';
import { CommandRejected } from '../../../document/commands';
import type { Predicate, Value, WorkflowDocument } from '../../../document/model';
import { useFieldDraft } from './drafts';
import { JsonField } from './fields';

type Edit = (apply: (document: WorkflowDocument) => WorkflowDocument, coalesce?: string) => void;

/** Change the value of one kind at `address` from its current value; a
 * value whose kind changed meanwhile is not this field any more. */
function updateKind<K extends Value['type'] | Predicate['type'], T extends { type: string }>(document: WorkflowDocument, address: FieldAddress, kinds: readonly K[], update: (current: Extract<T, { type: K }>) => T): WorkflowDocument {
  return updateField<T>(document, address, current => {
    if (!kinds.includes(current.type as K)) throw new CommandRejected('missing_field');
    return update(current as Extract<T, { type: K }>);
  });
}
import css from '../Editor.module.css';

type ValueKind = Value['type'];
const VALUE_KINDS: readonly ValueKind[] = ['reference', 'literal', 'object', 'array'];

function template(kind: ValueKind): Value {
  switch (kind) {
    case 'reference': return { type: 'reference', path: ['args'] };
    case 'literal': return { type: 'literal', value: null };
    case 'object': return { type: 'object', fields: {} };
    case 'array': return { type: 'array', items: [] };
  }
}

/** The native value syntax at `address`: an explicit reference path,
 * literal JSON, or a constructed object or array. Nothing here is an
 * expression language. Every change is made from the current document. */
export function ValueEditor({ label, address, value, edit }: { label: string; address: FieldAddress; value: Value; edit: Edit }) {
  const tx = useTranslation();
  const change = <K extends ValueKind>(kinds: readonly K[], update: (current: Extract<Value, { type: K }>) => Value) => edit(document => updateKind<K, Value>(document, address, kinds, update));
  return <fieldset className={css.value}>
    <legend className={css.fieldLabel}>{label}</legend>
    <div className={css.kindRow} role="radiogroup" aria-label={tx('workflows:value.kind')}>
      {VALUE_KINDS.map(kind => <button key={kind} type="button" role="radio" aria-checked={value.type === kind} className={clsx(css.chip, value.type === kind && css.chipActive)}
        onClick={() => { if (value.type !== kind) edit(document => setField(document, address, template(kind))); }}>{tx(`workflows:value.${kind}`)}</button>)}
    </div>
    {value.type === 'reference' && <PathEditor address={within(address, ['path'])} path={value.path} edit={edit} />}
    {value.type === 'literal' && <JsonField label={tx('workflows:value.literal-json')} value={value.value} rows={3} address={within(address, ['value'])} />}
    {value.type === 'object' && <div className={css.nested}>
      {Object.entries(value.fields).map(([key, entry]) => <div key={key} className={css.nestedRow}>
        <ValueEditor label={key} address={within(address, ['fields', { key }])} value={entry} edit={edit} />
        <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: key })}
          onClick={() => change(['object'], current => ({ ...current, fields: Object.fromEntries(Object.entries(current.fields).filter(([name]) => name !== key)) }))}>×</button>
      </div>)}
      <AddKey existing={Object.keys(value.fields)} onAdd={key => change(['object'], current => Object.hasOwn(current.fields, key) ? current : { ...current, fields: { ...current.fields, [key]: template('reference') } })} />
    </div>}
    {value.type === 'array' && <div className={css.nested}>
      {value.items.map((item, index) => <div key={index} className={css.nestedRow}>
        <ValueEditor label={`[${index}]`} address={within(address, ['items', index])} value={item} edit={edit} />
        <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: `[${index}]` })}
          onClick={() => change(['array'], current => ({ ...current, items: current.items.filter((_, at) => at !== index) }))}>×</button>
      </div>)}
      <button type="button" className={css.smallButton} onClick={() => change(['array'], current => ({ ...current, items: [...current.items, template('reference')] }))}>{tx('workflows:inspector.add-item')}</button>
    </div>}
  </fieldset>;
}

/** Explicit path components at the path array `address`; a component may
 * itself contain any text. A component being added is unapplied text of
 * the field `address` with the `append` role. */
function PathEditor({ address, path, edit }: { address: FieldAddress; path: readonly string[]; edit: Edit }) {
  const tx = useTranslation();
  const adding = useFieldDraft({
    address: { ...address, role: 'append' }, canonical: '',
    command: (document, text) => {
      const current = fieldValue(document, address);
      if (!current || !Array.isArray(current.value)) return { status: 'rejected', error: { kind: 'missing' } };
      return { status: 'applied', document: setField(document, address, [...current.value, text]) };
    },
  });
  const set = (update: (current: string[]) => string[], coalesce?: string) => edit(document => updateField<string[]>(document, address, current => {
    if (!Array.isArray(current)) throw new CommandRejected('missing_field');
    return update(current);
  }), coalesce);
  return <div className={css.path} aria-label={tx('workflows:value.path')} role="group">
    {path.map((component, index) => <span key={index} className={css.pathComponent}>
      <input className={clsx(css.input, css.mono, css.pathInput)} value={component} aria-label={`${tx('workflows:value.path')} ${index + 1}`} spellCheck={false}
        size={Math.max(4, component.length)} onChange={event => { const text = event.target.value; set(current => current.map((value, at) => at === index ? text : value), encodeField(within(address, [index]))); }} />
      <button type="button" className={css.pathRemove} aria-label={tx('workflows:inspector.remove', { p0: component })} onClick={() => set(current => current.filter((_, at) => at !== index))}>×</button>
    </span>)}
    <input className={clsx(css.input, css.mono, css.pathInput)} value={adding.text} placeholder="+" aria-label={tx('workflows:value.path-add')} aria-invalid={!!adding.error || undefined} spellCheck={false} size={6}
      onChange={event => adding.change(event.target.value)} onBlur={adding.apply} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); adding.apply(); } else if (event.key === 'Escape') adding.reset(); }} />
    <small className={css.hint}>{tx('workflows:value.path-hint')}</small>
  </div>;
}

export function AddKey({ existing, onAdd, label, action }: { existing: readonly string[]; onAdd: (key: string) => void; label?: string; action?: string }) {
  const tx = useTranslation();
  const [key, setKey] = useState('');
  const valid = key !== '' && !existing.includes(key);
  const add = () => { if (valid) { onAdd(key); setKey(''); } };
  return <div className={css.inline}>
    <input className={clsx(css.input, css.mono)} value={key} placeholder={label ?? tx('workflows:inspector.field-key')} aria-label={label ?? tx('workflows:inspector.field-key')} spellCheck={false}
      onChange={event => setKey(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); add(); } }} />
    <button type="button" className={css.smallButton} disabled={!valid} onClick={add}>{action ?? tx('workflows:inspector.add-field')}</button>
  </div>;
}

type PredicateKind = Predicate['type'];
const PREDICATE_KINDS: readonly PredicateKind[] = ['boolean', 'equal', 'not_equal', 'not', 'and', 'or'];

function predicateTemplate(kind: PredicateKind, from: Predicate): Predicate {
  const operand: Value = from.type === 'boolean' ? from.value : from.type === 'equal' || from.type === 'not_equal' ? from.left : { type: 'reference', path: ['args'] };
  switch (kind) {
    case 'boolean': return { type: 'boolean', value: operand };
    case 'equal':
    case 'not_equal': return { type: kind, left: operand, right: { type: 'literal', value: null } };
    case 'not': return { type: 'not', predicate: from };
    case 'and':
    case 'or': return { type: kind, predicates: [from] };
  }
}

/** The native predicate vocabulary at `address`; equality compares
 * same-typed scalars. */
export function PredicateEditor({ label, address, predicate, edit }: { label: string; address: FieldAddress; predicate: Predicate; edit: Edit }) {
  const tx = useTranslation();
  const change = <K extends PredicateKind>(kinds: readonly K[], update: (current: Extract<Predicate, { type: K }>) => Predicate) => edit(document => updateKind<K, Predicate>(document, address, kinds, update));
  return <fieldset className={css.value}>
    <legend className={css.fieldLabel}>{label}</legend>
    <select className={css.input} aria-label={tx('workflows:predicate.kind')} value={predicate.type}
      onChange={event => { const kind = event.target.value as PredicateKind; if (kind !== predicate.type) edit(document => updateField<Predicate>(document, address, current => current.type === kind ? current : predicateTemplate(kind, current))); }}>
      {PREDICATE_KINDS.map(kind => <option key={kind} value={kind}>{tx(`workflows:predicate.${kind}`)}</option>)}
    </select>
    {predicate.type === 'boolean' && <ValueEditor label={tx('workflows:predicate.boolean')} address={within(address, ['value'])} value={predicate.value} edit={edit} />}
    {(predicate.type === 'equal' || predicate.type === 'not_equal') && <>
      <ValueEditor label={tx('workflows:predicate.left')} address={within(address, ['left'])} value={predicate.left} edit={edit} />
      <ValueEditor label={tx('workflows:predicate.right')} address={within(address, ['right'])} value={predicate.right} edit={edit} />
    </>}
    {predicate.type === 'not' && <div className={css.nested}><PredicateEditor label={tx('workflows:predicate.not')} address={within(address, ['predicate'])} predicate={predicate.predicate} edit={edit} /></div>}
    {(predicate.type === 'and' || predicate.type === 'or') && <div className={css.nested}>
      {predicate.predicates.map((item, index) => <div key={index} className={css.nestedRow}>
        <PredicateEditor label={`${index + 1}`} address={within(address, ['predicates', index])} predicate={item} edit={edit} />
        <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: String(index + 1) })} disabled={predicate.predicates.length === 1}
          onClick={() => change(['and', 'or'], current => current.predicates.length === 1 ? current : { ...current, predicates: current.predicates.filter((_, at) => at !== index) })}>×</button>
      </div>)}
      <button type="button" className={css.smallButton} onClick={() => change(['and', 'or'], current => ({ ...current, predicates: [...current.predicates, { type: 'boolean', value: { type: 'reference', path: ['args'] } }] }))}>{tx('workflows:predicate.add')}</button>
    </div>}
  </fieldset>;
}
