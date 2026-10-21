import { useState } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
import type { Predicate, Value } from '../../../document/model';
import { JsonField } from './fields';
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

/** The native value syntax: an explicit reference path, literal JSON, or a
 * constructed object or array. Nothing here is an expression language. */
export function ValueEditor({ label, value, onChange }: { label: string; value: Value; onChange: (value: Value) => void }) {
  const tx = useTranslation();
  return <fieldset className={css.value}>
    <legend className={css.fieldLabel}>{label}</legend>
    <div className={css.kindRow} role="radiogroup" aria-label={tx('workflows:value.kind')}>
      {VALUE_KINDS.map(kind => <button key={kind} type="button" role="radio" aria-checked={value.type === kind} className={clsx(css.chip, value.type === kind && css.chipActive)}
        onClick={() => { if (value.type !== kind) onChange(template(kind)); }}>{tx(`workflows:value.${kind}`)}</button>)}
    </div>
    {value.type === 'reference' && <PathEditor path={value.path} onChange={path => onChange({ ...value, path })} />}
    {value.type === 'literal' && <JsonField label={tx('workflows:value.literal-json')} value={value.value} rows={3} onApply={literal => onChange({ ...value, value: literal })} />}
    {value.type === 'object' && <div className={css.nested}>
      {Object.entries(value.fields).map(([key, field]) => <div key={key} className={css.nestedRow}>
        <ValueEditor label={key} value={field} onChange={next => onChange({ ...value, fields: { ...value.fields, [key]: next } })} />
        <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: key })}
          onClick={() => onChange({ ...value, fields: Object.fromEntries(Object.entries(value.fields).filter(([name]) => name !== key)) })}>×</button>
      </div>)}
      <AddKey existing={Object.keys(value.fields)} onAdd={key => onChange({ ...value, fields: { ...value.fields, [key]: template('reference') } })} />
    </div>}
    {value.type === 'array' && <div className={css.nested}>
      {value.items.map((item, index) => <div key={index} className={css.nestedRow}>
        <ValueEditor label={`[${index}]`} value={item} onChange={next => onChange({ ...value, items: value.items.map((current, at) => at === index ? next : current) })} />
        <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: `[${index}]` })}
          onClick={() => onChange({ ...value, items: value.items.filter((_, at) => at !== index) })}>×</button>
      </div>)}
      <button type="button" className={css.smallButton} onClick={() => onChange({ ...value, items: [...value.items, template('reference')] })}>{tx('workflows:inspector.add-item')}</button>
    </div>}
  </fieldset>;
}

/** Explicit path components; a component may itself contain any text. */
function PathEditor({ path, onChange }: { path: readonly string[]; onChange: (path: string[]) => void }) {
  const tx = useTranslation();
  const [draft, setDraft] = useState('');
  const add = () => { if (draft) { onChange([...path, draft]); setDraft(''); } };
  return <div className={css.path} aria-label={tx('workflows:value.path')} role="group">
    {path.map((component, index) => <span key={index} className={css.pathComponent}>
      <input className={clsx(css.input, css.mono, css.pathInput)} value={component} aria-label={`${tx('workflows:value.path')} ${index + 1}`} spellCheck={false}
        size={Math.max(4, component.length)} onChange={event => onChange(path.map((current, at) => at === index ? event.target.value : current))} />
      <button type="button" className={css.pathRemove} aria-label={tx('workflows:inspector.remove', { p0: component })} onClick={() => onChange(path.filter((_, at) => at !== index))}>×</button>
    </span>)}
    <input className={clsx(css.input, css.mono, css.pathInput)} value={draft} placeholder="+" aria-label={tx('workflows:value.path-add')} spellCheck={false} size={6}
      onChange={event => setDraft(event.target.value)} onBlur={add} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); add(); } }} />
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

/** The native predicate vocabulary; equality compares same-typed scalars. */
export function PredicateEditor({ label, predicate, onChange }: { label: string; predicate: Predicate; onChange: (predicate: Predicate) => void }) {
  const tx = useTranslation();
  return <fieldset className={css.value}>
    <legend className={css.fieldLabel}>{label}</legend>
    <select className={css.input} aria-label={tx('workflows:predicate.kind')} value={predicate.type}
      onChange={event => { const kind = event.target.value as PredicateKind; if (kind !== predicate.type) onChange(predicateTemplate(kind, predicate)); }}>
      {PREDICATE_KINDS.map(kind => <option key={kind} value={kind}>{tx(`workflows:predicate.${kind}`)}</option>)}
    </select>
    {predicate.type === 'boolean' && <ValueEditor label={tx('workflows:predicate.boolean')} value={predicate.value} onChange={value => onChange({ ...predicate, value })} />}
    {(predicate.type === 'equal' || predicate.type === 'not_equal') && <>
      <ValueEditor label={tx('workflows:predicate.left')} value={predicate.left} onChange={left => onChange({ ...predicate, left })} />
      <ValueEditor label={tx('workflows:predicate.right')} value={predicate.right} onChange={right => onChange({ ...predicate, right })} />
    </>}
    {predicate.type === 'not' && <div className={css.nested}><PredicateEditor label={tx('workflows:predicate.not')} predicate={predicate.predicate} onChange={inner => onChange({ ...predicate, predicate: inner })} /></div>}
    {(predicate.type === 'and' || predicate.type === 'or') && <div className={css.nested}>
      {predicate.predicates.map((item, index) => <div key={index} className={css.nestedRow}>
        <PredicateEditor label={`${index + 1}`} predicate={item} onChange={next => onChange({ ...predicate, predicates: predicate.predicates.map((current, at) => at === index ? next : current) })} />
        <button type="button" className={css.smallButton} aria-label={tx('workflows:inspector.remove', { p0: String(index + 1) })} disabled={predicate.predicates.length === 1}
          onClick={() => onChange({ ...predicate, predicates: predicate.predicates.filter((_, at) => at !== index) })}>×</button>
      </div>)}
      <button type="button" className={css.smallButton} onClick={() => onChange({ ...predicate, predicates: [...predicate.predicates, { type: 'boolean', value: { type: 'reference', path: ['args'] } }] })}>{tx('workflows:predicate.add')}</button>
    </div>}
  </fieldset>;
}
