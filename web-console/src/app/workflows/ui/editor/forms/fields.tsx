import { useId, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
import { encodeField, fieldText, setField, type FieldAddress } from '../../../document/fields';
import type { FieldCommand, FieldError } from '../../../machines/editor';
import { useFieldDraft } from './drafts';
import { readJson } from '../../../../../../shared/json-numbers';
import css from '../Editor.module.css';

export function Field({ label, hint, children, error }: { label: string; hint?: ReactNode; children: (id: string, describedBy?: string) => ReactNode; error?: string }) {
  const id = useId(), hintId = `${id}-hint`;
  return <div className={css.field}>
    <label htmlFor={id} className={css.fieldLabel}>{label}</label>
    {children(id, hint || error ? hintId : undefined)}
    {(error || hint) && <small id={hintId} className={clsx(css.hint, error && css.hintError)} role={error ? 'alert' : undefined}>{error ?? hint}</small>}
  </div>;
}

/** A text control bound to one document field, named by its encoded
 * identity `field` (`encodeField`, or `workflowField` for the Workflow's
 * own fields). Every change is a semantic edit; one focus session of that
 * field coalesces into one undo step. */
export function TextField({ label, hint, value, field, onChange, multiline, mono, placeholder, list, invalid }: {
  label: string; hint?: ReactNode; value: string; field: string; onChange: (value: string, session: string) => void; multiline?: boolean; mono?: boolean; placeholder?: string; list?: string; invalid?: boolean;
}) {
  const [session, setSession] = useState(0);
  const key = field;
  return <Field label={label} hint={hint}>{(id, describedBy) => multiline
    ? <textarea id={id} data-workflow-field={field} aria-describedby={describedBy} aria-invalid={invalid || undefined} className={clsx(css.input, css.textarea, mono && css.mono)} value={value} placeholder={placeholder}
      onFocus={() => setSession(value => value + 1)} onChange={event => onChange(event.target.value, `${key}:${session}`)} />
    : <input id={id} data-workflow-field={field} aria-describedby={describedBy} aria-invalid={invalid || undefined} className={clsx(css.input, mono && css.mono)} value={value} placeholder={placeholder} list={list} spellCheck={false}
      onFocus={() => setSession(value => value + 1)} onChange={event => onChange(event.target.value, `${key}:${session}`)} />}
  </Field>;
}

/** The author-facing explanation of a field error. */
export function useFieldError() {
  const tx = useTranslation();
  return (error: FieldError | undefined) => {
    switch (error?.kind) {
      case undefined: return undefined;
      case 'syntax': return tx('workflows:inspector.json-invalid');
      case 'number': return tx('workflows:inspector.json-number');
      case 'range': return tx('workflows:inspector.number-range', { p0: error.min, p1: error.max });
      case 'empty': return tx('workflows:inspector.field-empty');
      case 'changed': return tx('workflows:inspector.field-changed');
      case 'missing': return tx('workflows:inspector.field-missing');
      case 'occupied': return tx('workflows:inspector.field-occupied');
      case 'rejected': return tx(`workflows:rejection.${error.code}`);
    }
  };
}

/** Resolution of a draft whose field changed under it: keep the text over
 * the current value, or take the current value. */
function ConflictActions({ field }: { field: { conflict?: 'changed' | 'missing'; keep: () => void; reset: () => void } }) {
  const tx = useTranslation();
  if (!field.conflict) return null;
  return <div className={css.inline}>
    {field.conflict === 'changed' && <button type="button" className={css.smallButton} onClick={field.keep}>{tx('workflows:inspector.field-keep')}</button>}
    <button type="button" className={css.smallButton} onClick={field.reset}>{tx(field.conflict === 'changed' ? 'workflows:inspector.field-current' : 'workflows:inspector.field-discard')}</button>
  </div>;
}

/** Apply JSON text at a `value` address of the current document. */
export function jsonCommand(address: FieldAddress): FieldCommand {
  return (document, text) => {
    const reading = readJson(text);
    if (!reading.ok) return { status: 'rejected', error: { kind: reading.reason } };
    const current = fieldText(document, address);
    if (current === undefined) return { status: 'rejected', error: { kind: 'missing' } };
    if ((JSON.stringify(reading.value, null, 2) ?? 'null') === current) return { status: 'unchanged' };
    return { status: 'applied', document: setField(document, address, reading.value) };
  };
}

/** A JSON value edited as text at `address`. Its unapplied text belongs to
 * the editor (see `useFieldDraft`); blur, Ctrl/Cmd+Enter, Save or Check
 * request its application, which the editor accepts only when it parses
 * within the browser number contract. */
export function JsonField({ label, hint, value, rows = 6, address }: { label: string; hint?: ReactNode; value: unknown; rows?: number; address: FieldAddress }) {
  const describe = useFieldError();
  const field = useFieldDraft({ address, canonical: JSON.stringify(value, null, 2) ?? 'null', command: jsonCommand(address) });
  return <Field label={label} hint={hint} error={describe(field.error)}>{(id, describedBy) => <>
    <textarea id={id} data-workflow-field={encodeField(address)} aria-describedby={describedBy} aria-invalid={!!field.error || undefined} className={clsx(css.input, css.textarea, css.mono)} rows={rows} value={field.text} spellCheck={false}
      onChange={event => field.change(event.target.value)} onBlur={field.apply}
      onKeyDown={event => { if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) { event.preventDefault(); field.apply(); } }} />
    <ConflictActions field={field} />
  </>}</Field>;
}

/** Plain metadata text uses the same versioned draft owner as JSON. Encoding
 * keeps its canonical field identity exact even for empty strings and quotes. */
export function DraftTextField({ label, value, address, multiline, compact }: { label: string; value: string; address: FieldAddress; multiline?: boolean; compact?: boolean }) {
  const describe = useFieldError();
  const field = useFieldDraft({ address, canonical: JSON.stringify(value), command: jsonCommand(address) });
  let text = field.text;
  try { const parsed: unknown = JSON.parse(field.text); if (typeof parsed === 'string') text = parsed; } catch { /* Kept detached text remains inspectable. */ }
  const control = (id?: string, describedBy?: string) => multiline
    ? <textarea id={id} data-workflow-field={encodeField(address)} aria-label={compact ? label : undefined} aria-describedby={describedBy} className={clsx(css.input, css.textarea)} value={text} aria-invalid={!!field.error || undefined} onChange={event => field.change(JSON.stringify(event.target.value))} onBlur={field.apply} />
    : <input id={id} data-workflow-field={encodeField(address)} aria-label={compact ? label : undefined} aria-describedby={describedBy} className={compact ? css.titleInput : css.input} value={text} aria-invalid={!!field.error || undefined} onChange={event => field.change(JSON.stringify(event.target.value))} onBlur={field.apply} onKeyDown={event => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) field.apply(); }} />;
  return compact ? <>{control()}{field.error && <small className={css.hintError} role="alert">{describe(field.error)}</small>}<ConflictActions field={field} /></> : <Field label={label} error={describe(field.error)}>{(id, describedBy) => <>{control(id, describedBy)}<ConflictActions field={field} /></>}</Field>;
}

/** Incomplete or out-of-range numbers remain drafts across panel changes. */
export function NumberField({ label, value, address, min = 0, max = Number.MAX_SAFE_INTEGER, optional = false, hint }: { label: string; value?: number | null; address: FieldAddress; min?: number; max?: number; optional?: boolean; hint?: ReactNode }) {
  const describe = useFieldError(), location = optional ? { ...address, role: 'optional' as const } : address;
  const field = useFieldDraft({ address: location, canonical: value == null ? '' : String(value), command: (document, text) => {
    if (optional && text === '') return { status: 'applied', document: setField(document, location, undefined) };
    if (!text) return { status: 'rejected', error: { kind: 'empty' } };
    if (!/^\d+$/.test(text)) return { status: 'rejected', error: { kind: 'range', min, max } };
    const number = Number(text);
    if (!Number.isSafeInteger(number)) return { status: 'rejected', error: { kind: 'number' } };
    if (number < min || number > max) return { status: 'rejected', error: { kind: 'range', min, max } };
    return { status: 'applied', document: setField(document, location, number) };
  } });
  return <Field label={label} hint={hint} error={describe(field.error)}>{(id, describedBy) => <><input id={id} data-workflow-field={encodeField(location)} className={css.input} type="text" inputMode="numeric" value={field.text} aria-describedby={describedBy} aria-invalid={!!field.error || undefined}
    onChange={event => field.change(event.target.value)} onBlur={field.apply} onKeyDown={event => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) field.apply(); }} /><ConflictActions field={field} /></>}</Field>;
}

/** An authored key at a `key` address (a node id, a Parallel branch key),
 * renamed explicitly on Enter, blur, Apply, Save or Check through `rename`,
 * never per keystroke. `onRenamed` runs once the editor accepted it. */
export function KeyField({ label, hint, value, address, rename, onRenamed, error, compact }: {
  label: string; hint?: ReactNode; value: string; address: FieldAddress; error?: string; compact?: string;
  rename: (document: Parameters<FieldCommand>[0], from: string, to: string) => ReturnType<typeof setField>; onRenamed?: (to: string) => void;
}) {
  const tx = useTranslation();
  const describe = useFieldError();
  const field = useFieldDraft({
    address, canonical: value, onApplied: text => onRenamed?.(text.trim()),
    command: (document, text) => {
      const to = text.trim(), from = fieldText(document, address);
      if (from === undefined) return { status: 'rejected', error: { kind: 'missing' } };
      if (!to) return { status: 'rejected', error: { kind: 'empty' } };
      if (to === from) return { status: 'unchanged' };
      return { status: 'applied', document: rename(document, from, to) };
    },
  });
  const shown = describe(field.error) ?? error;
  const input = (id?: string, describedBy?: string) => <input id={id} data-workflow-field={encodeField(address)} aria-describedby={describedBy} aria-label={compact} aria-invalid={!!field.error || !!error || undefined} className={clsx(css.input, css.mono, compact && css.branchKey)} value={field.text} spellCheck={false}
    onChange={event => field.change(event.target.value)} onBlur={field.apply}
    onKeyDown={event => { if (event.nativeEvent.isComposing) return; if (event.key === 'Enter') { event.preventDefault(); field.apply(); } else if (event.key === 'Escape') field.reset(); }} />;
  if (compact) return <span className={css.keyField}>{input()}{shown && <small className={clsx(css.hint, css.hintError)} role="alert">{shown}</small>}<ConflictActions field={field} /></span>;
  return <Field label={label} hint={hint} error={shown}>{(id, describedBy) => <><div className={css.inline}>
    {input(id, describedBy)}
    <button type="button" className={css.smallButton} disabled={!field.pending} onClick={field.apply}>{tx('workflows:inspector.rename')}</button>
  </div><ConflictActions field={field} /></>}</Field>;
}

export function SelectField<T extends string>({ label, value, options, onChange, hint, disabled, field }: { field?: string; label: string; value: T; options: readonly { value: T; label: string }[]; onChange: (value: T) => void; hint?: ReactNode; disabled?: boolean }) {
  return <Field label={label} hint={hint}>{(id, describedBy) => <select id={id} data-workflow-field={field} aria-describedby={describedBy} className={css.input} disabled={disabled} value={value} onChange={event => onChange(event.target.value as T)}>
    {options.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
  </select>}</Field>;
}

export function Section({ title, aside, children }: { title: string; aside?: ReactNode; children: ReactNode }) {
  return <section className={css.group}>
    <h3 className={css.groupHeading}>{title}{aside !== undefined && <small>{aside}</small>}</h3>
    {children}
  </section>;
}
