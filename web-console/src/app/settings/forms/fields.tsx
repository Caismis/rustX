import { useId, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { Button } from '../../../presentation/primitives/Button';
import { Choice, Toggle } from '../primitives/aria';
import type { TypedUnitForm } from './bridge';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The typed field layer over one unit's TanStack Form instance.
 *
 * These components own field mechanics only: reading and writing one typed
 * path of a nested authored document, the local touched/dirty metadata a form
 * needs to present itself, and syntactic validation.
 *
 * Validation here is deliberately narrow — a required identity, a parseable
 * URL, a positive number, a well-formed JSON scalar. Native Rust remains the
 * authority on configuration semantics, source validity, Provider/Model
 * legality, capability legality, policy legality and exact mutation
 * acceptance. There is no browser mirror of the native configuration schema,
 * and no native-valid configuration is refused here. */

interface BoundField { state: { value: unknown; meta: { errors: unknown[]; isTouched: boolean } }; handleChange: (value: never) => void; handleBlur: () => void; name: string }

/** A path into one authored document.
 *
 * TanStack's own `DeepKeys<T>` cannot be used for these documents: several of
 * them embed `RequestParamsToml`, whose values are arbitrarily nested JSON, so
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
  const errors = field.state.meta.errors.filter(Boolean);
  if (!field.state.meta.isTouched || !errors.length) return null;
  return <span role="alert" className={css.error}>{errors.map(error => String((error as { message?: string })?.message ?? error)).join('; ')}</span>;
}

/** Syntactic checks a browser can make truthfully. Each returns a message or
 * `undefined`; none of them decides whether native would accept the value. */
export const syntactic = {
  required: (label: string) => (value: unknown) => (value === undefined || value === null || String(value).trim() === '') ? `${label} is required.` : undefined,
  url: (value: unknown) => { if (!value) return undefined; try { new URL(String(value)); return undefined; } catch { return 'Enter a complete URL, including its scheme.'; } },
  positive: (label: string) => (value: unknown) => value === undefined || value === null || value === '' ? undefined : Number(value) > 0 ? undefined : `${label} must be a positive number.`,
};

export function Text<T>({ form, name, label, required = false, url = false, placeholder }: {
  form: TypedUnitForm<T>; name: FieldPath<T>; label: string; required?: boolean; url?: boolean; placeholder?: string;
}) {
  const validate = (value: unknown) => (required ? syntactic.required(label)(value) : undefined) ?? (url ? syntactic.url(value) : undefined);
  const Field = form.Field as unknown as (props: { name: string; validators: { onChange: (props: { value: unknown }) => string | undefined }; children: (field: BoundField) => ReactNode }) => ReactNode;
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
  const Field = form.Field as unknown as (props: { name: string; validators: { onChange: (props: { value: unknown }) => string | undefined }; children: (field: BoundField) => ReactNode }) => ReactNode;
  return <Field name={name as string} validators={{ onChange: ({ value }) => syntactic.positive(label)(value) }}>{field => <label>{label}
    <input type="number" min={min} value={(field.state.value as number | string | null | undefined) ?? ''}
      aria-invalid={field.state.meta.errors.filter(Boolean).length > 0 || undefined}
      onBlur={field.handleBlur} onChange={event => field.handleChange((event.target.value ? Number(event.target.value) : empty) as never)} />
    <Errors field={field} />
  </label>}</Field>;
}

/** A numeric field native models as a decimal string, kept as a string so no
 * precision is lost in the browser. */
export function NumericText<T>({ form, name, label }: { form: TypedUnitForm<T>; name: FieldPath<T>; label: string }) {
  const Field = form.Field as unknown as (props: { name: string; validators: { onChange: (props: { value: unknown }) => string | undefined }; children: (field: BoundField) => ReactNode }) => ReactNode;
  return <Field name={name as string} validators={{ onChange: ({ value }) => syntactic.positive(label)(value) }}>{field => <label>{label}
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
  return <Bound<T, boolean | null | undefined> form={form} name={name}>{(value, change) =>
    <Choice label={label} value={value === undefined || value === null ? '' : String(value)}
      options={[['', 'Native default'], ['true', 'On'], ['false', 'Off']]}
      onChange={next => change(next === '' ? undefined : next === 'true')} />}</Bound>;
}

/** A list of identities. An empty list is an authored empty list and is never
 * presented as an absent value. */
export function Strings<T>({ form, name, label }: { form: TypedUnitForm<T>; name: FieldPath<T>; label: string }) {
  const Field = form.Field as unknown as (props: { name: string; mode: 'array'; children: (field: BoundField & { pushValue: (value: never) => void; removeValue: (index: number) => void }) => ReactNode }) => ReactNode;
  return <Field name={name as string} mode="array">{field => {
    const value = (field.state.value as string[] | null | undefined) ?? [];
    return <fieldset><legend>{label}</legend>
      {value.map((entry, index) => <div className={css.names} key={index}>
        <input aria-label={`${label} ${index + 1}`} value={entry}
          onChange={event => field.handleChange(value.map((item, at) => at === index ? event.target.value : item) as never)} />
        <Button aria-label={`Remove ${label} ${index + 1}`} onClick={() => field.removeValue(index)}>Remove</Button>
      </div>)}
      <Button onClick={() => field.pushValue('' as never)}>Add {label}</Button>
      {!value.length && <p className={css.hint}>Empty list · no entries</p>}
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
  const [key, setKey] = useState('');
  const id = useId();
  const duplicate = key !== '' && key in value;
  return <fieldset><legend>{label}</legend>
    {Object.entries(value).map(([entry, current]) => <div key={entry}>
      <label>{entry}<input type={secret ? 'password' : 'text'} autoComplete={secret ? 'new-password' : undefined} value={current}
        onChange={event => change({ ...value, [entry]: event.target.value })} /></label>
      <Button onClick={() => { const next = { ...value }; delete next[entry]; change(next); }}>Remove {entry}</Button>
    </div>)}
    <label htmlFor={id}>{label} name</label>
    <input id={id} value={key} aria-invalid={duplicate || undefined} onChange={event => setKey(event.target.value)} />
    {duplicate && <span role="alert" className={css.error}>{key} is already listed.</span>}
    <Button disabled={!key || duplicate} onClick={() => { change({ ...value, [key]: '' }); setKey(''); }}>Add {label}</Button>
  </fieldset>;
}

/** Explicit request parameters: an open map whose values are JSON scalars,
 * arrays or objects. Native Rust validates protected keys and protocol
 * support; the browser only requires each entry to be complete JSON. */
export function RequestParameters<T>({ form, name }: { form: TypedUnitForm<T>; name: FieldPath<T> }) {
  return <Bound<T, Record<string, unknown> | null | undefined> form={form} name={name}>{(raw, change) =>
    <RequestParameterRows value={raw ?? {}} change={change} />}</Bound>;
}
export function RequestParameterRows({ value, change }: { value: Record<string, unknown>; change: (value: Record<string, unknown>) => void }) {
  const [key, setKey] = useState('');
  const duplicate = key !== '' && key in value;
  return <fieldset><legend>Explicit request parameters</legend>
    {Object.entries(value).map(([parameter, current]) => <div key={parameter}>
      <label>{parameter}<JsonValue value={current} change={next => change({ ...value, [parameter]: next })} /></label>
      <Button onClick={() => { const next = { ...value }; delete next[parameter]; change(next); }}>Remove {parameter}</Button>
    </div>)}
    <label>Parameter name<input value={key} aria-invalid={duplicate || undefined} onChange={event => setKey(event.target.value)} /></label>
    {duplicate && <span role="alert" className={css.error}>{key} is already a request parameter.</span>}
    <Button disabled={!key || duplicate} onClick={() => { change({ ...value, [key]: '' }); setKey(''); }}>Add parameter</Button>
    <p className={css.hint}>Each value is a JSON scalar, array, or object. Native Rust validates protected keys and protocol support. Do not enter secrets.</p>
  </fieldset>;
}
/** One request parameter's JSON value.
 *
 * The parsed value belongs to the unit's transaction actor, through the form.
 * The text is a transient buffer for exactly one thing the actor cannot hold:
 * input that is not yet a complete JSON value, such as `{"budget":`. Only a
 * complete value is ever emitted, so incomplete text stays local and never
 * reaches the draft.
 *
 * The buffer follows its owner. When `value` changes to anything other than
 * what this input itself last emitted, the owner changed it — a discarded
 * draft, a reviewed revision, a commit dropping the confirmed draft — and the
 * text, its parse error and the input's custom validity are replaced by the
 * owner's value. Following the owner never emits, so a stale buffer can never
 * be written back over a newer value. */
function JsonValue({ value, change }: { value: unknown; change: (value: unknown) => void }) {
  const serialized = JSON.stringify(value) ?? '';
  const [text, setText] = useState(serialized);
  const [error, setError] = useState('');
  const input = useRef<HTMLInputElement>(null);
  // The serialized value this buffer currently reflects: the owner's, or the
  // one this input last emitted into it.
  const reflected = useRef(serialized);
  useLayoutEffect(() => {
    if (serialized === reflected.current) return;
    reflected.current = serialized;
    setText(serialized);
    setError('');
  }, [serialized]);
  // Custom validity is a projection of the local parse error, so it is cleared
  // by exactly the transitions that clear the error.
  useLayoutEffect(() => { input.current?.setCustomValidity(error); }, [error]);
  return <>
    <input ref={input} value={text} aria-invalid={!!error || undefined} onChange={event => {
      const next = event.target.value;
      setText(next);
      let parsed: unknown;
      try { parsed = JSON.parse(next); } catch { setError('Enter a complete JSON value before saving.'); return; }
      setError('');
      reflected.current = JSON.stringify(parsed);
      change(parsed);
    }} />
    {error && <span role="alert">{error}</span>}
  </>;
}
