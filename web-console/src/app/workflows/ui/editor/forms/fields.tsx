import { useEffect, useId, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../../../../locale/react';
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

/** A JSON value edited as text. It is applied only when it parses, and
 * the document is never changed by unparseable text. */
export function JsonField({ label, hint, value, onApply, rows = 6 }: { label: string; hint?: ReactNode; value: unknown; onApply: (value: unknown) => void; rows?: number }) {
  const tx = useTranslation();
  const canonical = JSON.stringify(value, null, 2) ?? 'null';
  const [text, setText] = useState(canonical);
  const [error, setError] = useState(false);
  // A document change elsewhere (undo, rename, reload) replaces the draft text.
  useEffect(() => { setText(canonical); setError(false); }, [canonical]);
  const apply = () => {
    let parsed: unknown;
    try { parsed = JSON.parse(text); } catch { setError(true); return; }
    setError(false);
    if (JSON.stringify(parsed, null, 2) !== canonical) onApply(parsed);
  };
  return <Field label={label} hint={hint} error={error ? tx('workflows:inspector.json-invalid') : undefined}>{(id, describedBy) => <>
    <textarea id={id} aria-describedby={describedBy} aria-invalid={error || undefined} className={clsx(css.input, css.textarea, css.mono)} rows={rows} value={text} spellCheck={false}
      onChange={event => setText(event.target.value)} onBlur={apply}
      onKeyDown={event => { if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) { event.preventDefault(); apply(); } }} />
  </>}</Field>;
}

/** An identifier committed explicitly (Enter, blur or Apply), never per keystroke. */
export function IdentifierField({ label, hint, value, onCommit, error }: { label: string; hint?: ReactNode; value: string; onCommit: (value: string) => void; error?: string }) {
  const tx = useTranslation();
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const commit = () => { if (draft !== value) onCommit(draft.trim()); };
  return <Field label={label} hint={hint} error={error}>{(id, describedBy) => <div className={css.inline}>
    <input id={id} aria-describedby={describedBy} className={clsx(css.input, css.mono)} value={draft} spellCheck={false}
      onChange={event => setDraft(event.target.value)} onBlur={commit}
      onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); commit(); } else if (event.key === 'Escape') setDraft(value); }} />
    <button type="button" className={css.smallButton} disabled={draft === value} onClick={commit}>{tx('workflows:inspector.rename')}</button>
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
