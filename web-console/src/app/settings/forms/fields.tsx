import { message, displayText, type DisplayText } from '../../../locale/translation';
import { useTranslation, useNotice } from '../../../locale/react';
import { useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { Button } from '../../../presentation/primitives/Button';
import { Choice, Toggle } from '../primitives/aria';
import type { TypedUnitForm } from './bridge';
import { formatRequestParams, parseRequestParams, sameJson, type RequestParams } from './request-params';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The typed field layer over one unit's TanStack Form instance.
 *
 * These components own field mechanics only: reading and writing one typed
 * path of a nested authored document, the local touched/dirty metadata a form
 * needs to present itself, and syntactic validation.
 *
 * Validation here is deliberately narrow — a required identity, a parseable
 * URL, a positive number, a well-formed JSON object. Native Rust remains the
 * authority on configuration semantics, source validity, Provider/Model
 * legality, capability legality, policy legality and exact mutation
 * acceptance. There is no browser mirror of the native configuration schema,
 * and no native-valid configuration is refused here. */

interface BoundField { state: { value: unknown; meta: { errors: unknown[]; isTouched: boolean } }; handleChange: (value: never) => void; handleBlur: () => void; name: string }

/** A path into one authored document.
 *
 * TanStack's own `DeepKeys<T>` cannot be used for these documents: several of
 * them embed `RequestParams`, whose values are arbitrarily nested JSON, so
 * the deep key expansion never terminates. This checks the top-level field
 * name — the part a rename actually breaks — and leaves the nested suffix
 * free, which is exactly as much as the type system can say here. */
export type FieldPath<T> = Extract<keyof T, string> | `${Extract<keyof T, string>}.${string}`;

/** The one place the TanStack generic surface is erased. Every exported field
 * below stays typed against the unit's own authored document. */
function Bound<T, V>({ form, name, children }: {
  form: TypedUnitForm<T>; name: FieldPath<T>;
  children: (value: V, change: (value: V) => void, field: BoundField) => ReactNode;
}) {
  const Field = form.Field as unknown as (props: { name: string; children: (field: BoundField) => ReactNode }) => ReactNode;
  return <Field name={name as string}>{field => children(field.state.value as V, value => field.handleChange(value as never), field)}</Field>;
}

function Errors({ field }: { field: BoundField }) {
  const tx = useTranslation();
  const errors = field.state.meta.errors.filter(Boolean);
  if (!field.state.meta.isTouched || !errors.length) return null;
  return <span role="alert" className={css.error}>{errors.map(error => displayText(tx, ((error as { message?: DisplayText })?.message ?? error) as DisplayText)).join('; ')}</span>;
}

/** Syntactic checks a browser can make truthfully. Each returns a message or
 * `undefined`; none of them decides whether native would accept the value. */
export const syntactic = {
  required: (value: unknown) => (value === undefined || value === null || String(value).trim() === '') ? message('settings:validation.required') : undefined,
  url: (value: unknown) => { if (!value) return undefined; try { new URL(String(value)); return undefined; } catch { return message('settings:validation.url'); } },
  positive: (value: unknown) => value === undefined || value === null || value === '' ? undefined : Number(value) > 0 ? undefined : message('settings:validation.positive'),
};

export function Text<T>({ form, name, label, required = false, url = false, placeholder }: {
  form: TypedUnitForm<T>; name: FieldPath<T>; label: string; required?: boolean; url?: boolean; placeholder?: string;
}) {
  const validate = (value: unknown) => (required ? syntactic.required(value) : undefined) ?? (url ? syntactic.url(value) : undefined);
  const Field = form.Field as unknown as (props: { name: string; validators: { onChange: (props: { value: unknown }) => DisplayText | undefined }; children: (field: BoundField) => ReactNode }) => ReactNode;
  return <Field name={name as string} validators={{ onChange: ({ value }) => validate(value) }}>{field => <label>{label}
    <input value={(field.state.value as string | null | undefined) ?? ''} placeholder={placeholder}
      aria-invalid={field.state.meta.errors.filter(Boolean).length > 0 || undefined}
      onBlur={field.handleBlur} onChange={event => field.handleChange(event.target.value as never)} />
    <Errors field={field} />
  </label>}</Field>;
}

/** A secret-bearing authored field. The value is memory-only: it is never
 * persisted, never echoed into a URL, never logged and never read back from
 * native. Autofill is disabled so no browser store retains it either. */
export function Secret<T>({ form, name, label, required = false }: {
  form: TypedUnitForm<T>; name: FieldPath<T>; label: string; required?: boolean;
}) {
  return <Bound<T, string | null | undefined> form={form} name={name}>{(value, change, field) => <label>{label}
    <input type="password" autoComplete="new-password" spellCheck={false} value={value ?? ''}
      required={required} onBlur={field.handleBlur} onChange={event => change(event.target.value)} />
  </label>}</Bound>;
}

export function Area<T>({ form, name, label }: { form: TypedUnitForm<T>; name: FieldPath<T>; label: string }) {
  return <Bound<T, string | null | undefined> form={form} name={name}>{(value, change, field) => <label>{label}
    <textarea value={value ?? ''} onBlur={field.handleBlur} onChange={event => change(event.target.value)} />
  </label>}</Bound>;
}

export function Numeric<T>({ form, name, label, min = 1, empty = null }: {
  form: TypedUnitForm<T>; name: FieldPath<T>; label: string; min?: number; empty?: null | undefined;
}) {
  const Field = form.Field as unknown as (props: { name: string; validators: { onChange: (props: { value: unknown }) => DisplayText | undefined }; children: (field: BoundField) => ReactNode }) => ReactNode;
  return <Field name={name as string} validators={{ onChange: ({ value }) => syntactic.positive(value) }}>{field => <label>{label}
    <input type="number" min={min} value={(field.state.value as number | string | null | undefined) ?? ''}
      aria-invalid={field.state.meta.errors.filter(Boolean).length > 0 || undefined}
      onBlur={field.handleBlur} onChange={event => field.handleChange((event.target.value ? Number(event.target.value) : empty) as never)} />
    <Errors field={field} />
  </label>}</Field>;
}

/** A numeric field native models as a decimal string, kept as a string so no
 * precision is lost in the browser. */
export function NumericText<T>({ form, name, label }: { form: TypedUnitForm<T>; name: FieldPath<T>; label: string }) {
  const Field = form.Field as unknown as (props: { name: string; validators: { onChange: (props: { value: unknown }) => DisplayText | undefined }; children: (field: BoundField) => ReactNode }) => ReactNode;
  return <Field name={name as string} validators={{ onChange: ({ value }) => syntactic.positive(value) }}>{field => <label>{label}
    <input type="number" min={1} value={(field.state.value as string | null | undefined) ?? ''}
      aria-invalid={field.state.meta.errors.filter(Boolean).length > 0 || undefined}
      onBlur={field.handleBlur} onChange={event => field.handleChange((event.target.value || null) as never)} />
    <Errors field={field} />
  </label>}</Field>;
}

export function Enum<T, V extends string>({ form, name, label, options, empty }: {
  form: TypedUnitForm<T>; name: FieldPath<T>; label: string;
  options: readonly (readonly [V, string])[];
  /** The label of the absent selection. Present exactly when the native unit
   * distinguishes "no authored value" from every value it admits. */
  empty?: string;
}) {
  const choices = empty === undefined ? options : ([['', empty], ...options] as readonly (readonly [string, string])[]);
  return <Bound<T, V | null | undefined> form={form} name={name}>{(value, change) =>
    <Choice label={label} value={(value ?? '') as string} options={choices}
      onChange={next => change((next === '' ? null : next) as V)} />}</Bound>;
}

export function Bool<T>({ form, name, label }: { form: TypedUnitForm<T>; name: FieldPath<T>; label: string }) {
  return <Bound<T, boolean | null | undefined> form={form} name={name}>{(value, change) =>
    <Toggle label={label} checked={!!value} onChange={change} />}</Bound>;
}

/** A three-state boolean: on, off, or no authored value at all. Absent is
 * never collapsed into `false`. */
export function TriBool<T>({ form, name, label }: { form: TypedUnitForm<T>; name: FieldPath<T>; label: string }) {
  const tx = useTranslation();
  return <Bound<T, boolean | null | undefined> form={form} name={name}>{(value, change) =>
    <Choice label={label} value={value === undefined || value === null ? '' : String(value)}
      options={[['', tx('settings:copy.native-default')], ['true', tx('settings:copy.on')], ['false', tx('settings:copy.off')]]}
      onChange={next => change(next === '' ? undefined : next === 'true')} />}</Bound>;
}

/** A list of identities. An empty list is an authored empty list and is never
 * presented as an absent value. */
export function Strings<T>({ form, name, label }: { form: TypedUnitForm<T>; name: FieldPath<T>; label: string }) {
  const tx = useTranslation();
  const Field = form.Field as unknown as (props: { name: string; mode: 'array'; children: (field: BoundField & { pushValue: (value: never) => void; removeValue: (index: number) => void }) => ReactNode }) => ReactNode;
  return <Field name={name as string} mode="array">{field => {
    const value = (field.state.value as string[] | null | undefined) ?? [];
    return <fieldset><legend>{label}</legend>
      {value.map((entry, index) => <div className={css.names} key={index}>
        <input aria-label={tx('settings:extension-detail.value-value', { p0: label, p1: index + 1 })} value={entry}
          onChange={event => field.handleChange(value.map((item, at) => at === index ? event.target.value : item) as never)} />
        <Button aria-label={tx('settings:controls.remove-value-value', { p0: label, p1: index + 1 })} onClick={() => field.removeValue(index)}>{tx('settings:controls.remove')}{' '}</Button>
      </div>)}
      <Button onClick={() => field.pushValue('' as never)}>{tx('settings:extensions-page.add')}{' '}{label}</Button>
      {!value.length && <p className={css.hint}>{tx('settings:controls.empty-list-no-entries')}</p>}
    </fieldset>;
  }}</Field>;
}

/** A `Record<string, string>` of named entries — MCP environment references,
 * headers and their literal counterparts.
 *
 * When `secret` is set the values are authored secrets: they are rendered as
 * password inputs, held in memory only, and never read back from native. */
export function Entries<T>({ form, name, label, secret = false }: {
  form: TypedUnitForm<T>; name: FieldPath<T>; label: string; secret?: boolean;
}) {
  return <Bound<T, Record<string, string> | null | undefined> form={form} name={name}>{(raw, change) =>
    <EntryRows label={label} value={raw ?? {}} change={change} secret={secret} />}</Bound>;
}
export function EntryRows({ label, value, change, secret = false }: {
  label: string; value: Record<string, string>; change: (value: Record<string, string>) => void; secret?: boolean;
}) {
  const tx = useTranslation();
  const [key, setKey] = useState('');
  const id = useId();
  const duplicate = key !== '' && key in value;
  return <fieldset><legend>{label}</legend>
    {Object.entries(value).map(([entry, current]) => <div key={entry}>
      <label>{entry}<input type={secret ? 'password' : 'text'} autoComplete={secret ? 'new-password' : undefined} value={current}
        onChange={event => change({ ...value, [entry]: event.target.value })} /></label>
      <Button onClick={() => { const next = { ...value }; delete next[entry]; change(next); }}>{tx('settings:controls.remove')}{' '}{entry}</Button>
    </div>)}
    <label htmlFor={id}>{label} {tx('settings:fields.name')}</label>
    <input id={id} value={key} aria-invalid={duplicate || undefined} onChange={event => setKey(event.target.value)} />
    {duplicate && <span role="alert" className={css.error}>{key} {tx('settings:fields.is-already-listed')}</span>}
    <Button disabled={!key || duplicate} onClick={() => { change({ ...value, [key]: '' }); setKey(''); }}>{tx('settings:extensions-page.add')}{' '}{label}</Button>
  </fieldset>;
}

/** Provider-native request parameters bound to one form path. */
export function RequestParameters<T>({ form, name, label, optional = true }: {
  form: TypedUnitForm<T>; name: FieldPath<T>; label: string; optional?: boolean;
}) {
  return <Bound<T, RequestParams | null | undefined> form={form} name={name}>{(raw, change) =>
    <RequestParamsEditor label={label} value={raw ?? undefined} optional={optional} change={change} />}</Bound>;
}

/** One provider-native request-parameter object, edited as a whole JSON
 * object: paste or type nested objects, arrays and `null` as the provider
 * expects them.
 *
 * The parsed object belongs to the unit's transaction actor, through the
 * form. The text is a transient buffer for exactly what the actor cannot
 * hold: text that is not one JSON object without repeated keys. Such text
 * stays visible with its diagnostic, the field's custom validity blocks the
 * form's submission, and nothing is emitted, so an invalid draft can never
 * become configuration. A whitespace or key-order edit of the same object is
 * not a change and begins no draft.
 *
 * The buffer follows its owner: when `value` becomes structurally different
 * from what this editor last reflected, the owner changed it — a discarded
 * draft, a reviewed revision, a commit — and the text, its diagnostic and the
 * custom validity are replaced by the owner's value. Following never emits.
 *
 * `optional` distinguishes an absent object (blank text) from an authored
 * `{}`. Native Rust remains the authority on protected keys, profile
 * ownership and every other semantic. */
export function RequestParamsEditor({ label, value, change, optional = true }: {
  label: string; value: RequestParams | undefined; change: (value: RequestParams | undefined) => void; optional?: boolean;
}) {
  const tx = useTranslation();
  const display = (current: RequestParams | undefined) => current === undefined ? '' : formatRequestParams(current);
  const [text, setText] = useState(() => display(value));
  const [error, setError] = useNotice();
  const input = useRef<HTMLTextAreaElement>(null);
  const reflected = useRef(value);
  useLayoutEffect(() => {
    if (sameJson(value, reflected.current)) return;
    reflected.current = value;
    setText(display(value));
    setError('');
  }, [value]);
  useLayoutEffect(() => { input.current?.setCustomValidity(error); }, [error]);
  const accept = (next: RequestParams | undefined) => {
    setError('');
    if (sameJson(next, reflected.current)) return;
    reflected.current = next;
    change(next);
  };
  return <div>
    <label>{label}<textarea ref={input} value={text} data-request-params="" style={{ fontFamily: 'var(--ds-font-family-code), monospace' }} rows={Math.min(12, Math.max(3, text.split('\n').length))} spellCheck={false}
      placeholder={tx(optional ? 'settings:fields.request-params-placeholder-optional' : 'settings:fields.request-params-placeholder-required')}
      aria-invalid={!!error || undefined} onChange={event => {
        const next = event.target.value;
        setText(next);
        if (next.trim() === '') { accept(optional ? undefined : {}); return; }
        const parsed = parseRequestParams(next);
        if (parsed.ok) { accept(parsed.value); return; }
        setError(parsed.error.kind === 'syntax' ? message('settings:fields.request-params-invalid-json')
          : parsed.error.kind === 'not_object' ? message('settings:fields.request-params-not-object')
            : message('settings:fields.request-params-duplicate-key', { p0: parsed.error.path }));
      }} /></label>
    {error && <span role="alert" className={css.error}>{error}</span>}
    <p className={css.hint}>{tx('settings:fields.request-params-hint')}</p>
  </div>;
}
