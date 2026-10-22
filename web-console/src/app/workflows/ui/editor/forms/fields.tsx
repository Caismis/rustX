import { useId, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
import { readJson, useFieldDraft } from './drafts';
import css from '../Editor.module.css';

export function Field({ label, hint, children, error }: { label: string; hint?: ReactNode; children: (id: string, describedBy?: string) => ReactNode; error?: string }) {
  const id = useId(), hintId = `${id}-hint`;
  return <div className={css.field}>
    <label htmlFor={id} className={css.fieldLabel}>{label}</label>
    {children(id, hint || error ? hintId : undefined)}
    {(error || hint) && <small id={hintId} className={clsx(css.hint, error && css.hintError)} role={error ? 'alert' : undefined}>{error ?? hint}</small>}
  </div>;
}

/** A text control bound to one document field. Every change is a semantic
 * edit; one focus session coalesces into one undo step. */
export function TextField({ label, hint, value, onChange, multiline, mono, placeholder, list, invalid }: {
  label: string; hint?: ReactNode; value: string; onChange: (value: string, session: string) => void; multiline?: boolean; mono?: boolean; placeholder?: string; list?: string; invalid?: boolean;
}) {
  const [session, setSession] = useState(0);
  const key = useId();
  return <Field label={label} hint={hint}>{(id, describedBy) => multiline
    ? <textarea id={id} aria-describedby={describedBy} aria-invalid={invalid || undefined} className={clsx(css.input, css.textarea, mono && css.mono)} value={value} placeholder={placeholder}
      onFocus={() => setSession(value => value + 1)} onChange={event => onChange(event.target.value, `${key}:${session}`)} />
    : <input id={id} aria-describedby={describedBy} aria-invalid={invalid || undefined} className={clsx(css.input, mono && css.mono)} value={value} placeholder={placeholder} list={list} spellCheck={false}
      onFocus={() => setSession(value => value + 1)} onChange={event => onChange(event.target.value, `${key}:${session}`)} />}
  </Field>;
}

/** A JSON value edited as text, under the field identity `draftKey`. Its
 * unapplied text belongs to the editor (see `useFieldDraft`); it is applied
 * on blur, Ctrl/Cmd+Enter, Save or Check, only when it parses within the
 * browser number contract, and never changes the document otherwise. */
export function JsonField({ label, hint, value, onApply, rows = 6, draftKey }: { label: string; hint?: ReactNode; value: unknown; onApply: (value: unknown) => void; rows?: number; draftKey: string }) {
  const tx = useTranslation();
  const canonical = JSON.stringify(value, null, 2) ?? 'null';
  const [touched, setTouched] = useState(false);
  const field = useFieldDraft({
    key: draftKey, canonical,
    valid: text => readJson(text).ok,
    commit: text => {
      const reading = readJson(text);
      if (!reading.ok) return false;
      if (JSON.stringify(reading.value, null, 2) !== canonical) onApply(reading.value);
      return true;
    },
  });
  const reading = field.invalid && (touched || field.flagged) ? readJson(field.text) : undefined;
  const error = reading && !reading.ok ? tx(reading.reason === 'number' ? 'workflows:inspector.json-number' : 'workflows:inspector.json-invalid') : undefined;
  const apply = () => { setTouched(true); field.apply(); };
  return <Field label={label} hint={hint} error={error}>{(id, describedBy) => <>
    <textarea id={id} aria-describedby={describedBy} aria-invalid={field.invalid || undefined} className={clsx(css.input, css.textarea, css.mono)} rows={rows} value={field.text} spellCheck={false}
      onChange={event => field.change(event.target.value)} onBlur={apply}
      onKeyDown={event => { if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) { event.preventDefault(); apply(); } }} />
  </>}</Field>;
}

/** An identifier committed explicitly (Enter, blur, Apply, Save or Check),
 * never per keystroke. Its unapplied text belongs to the editor. */
export function IdentifierField({ label, hint, value, onCommit, error, draftKey }: { label: string; hint?: ReactNode; value: string; onCommit: (value: string) => boolean; error?: string; draftKey: string }) {
  const tx = useTranslation();
  const field = useFieldDraft({ key: draftKey, canonical: value, valid: text => text.trim() !== '', commit: text => text.trim() === value || onCommit(text.trim()) });
  return <Field label={label} hint={hint} error={error}>{(id, describedBy) => <div className={css.inline}>
    <input id={id} aria-describedby={describedBy} aria-invalid={field.invalid || !!error || undefined} className={clsx(css.input, css.mono)} value={field.text} spellCheck={false}
      onChange={event => field.change(event.target.value)} onBlur={field.apply}
      onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); field.apply(); } else if (event.key === 'Escape') field.reset(); }} />
    <button type="button" className={css.smallButton} disabled={!field.pending} onClick={field.apply}>{tx('workflows:inspector.rename')}</button>
  </div>}</Field>;
}

export function SelectField<T extends string>({ label, value, options, onChange, hint }: { label: string; value: T; options: readonly { value: T; label: string }[]; onChange: (value: T) => void; hint?: ReactNode }) {
  return <Field label={label} hint={hint}>{(id, describedBy) => <select id={id} aria-describedby={describedBy} className={css.input} value={value} onChange={event => onChange(event.target.value as T)}>
    {options.map(option => <option key={option.value} value={option.value}>{option.label}</option>)}
  </select>}</Field>;
}

export function Section({ title, aside, children }: { title: string; aside?: ReactNode; children: ReactNode }) {
  return <section className={css.group}>
    <h3 className={css.groupHeading}>{title}{aside !== undefined && <small>{aside}</small>}</h3>
    {children}
  </section>;
}
