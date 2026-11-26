import { message, displayText, type DisplayText } from '../../../locale/translation';
import { SettingsNotice, useSettingsFeedback } from '../SettingsFeedback';
import { useTranslation, useNotice } from '../../../locale/react';
import { useContext, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { JsonCodeEditor } from '../../../presentation/primitives/JsonCodeEditor';
import { Toggle } from '../primitives/aria';
import { UnitReset, type TypedUnitForm } from './bridge';
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

function Errors({ field, label }: { field: BoundField; label: string }) {
  const tx = useTranslation();
  const errors = field.state.meta.errors.filter(Boolean);
  if (!field.state.meta.isTouched || !errors.length) return null;
  const message = [...new Set(errors.map(error => displayText(tx, ((error as { message?: DisplayText })?.message ?? error) as DisplayText)))].join(' ');
  return <SettingsNotice text={message.startsWith(label) ? message : `${label} · ${message}`} fallback={<span role="alert" className={css.error}>{message}</span>}/>;
}

/** Syntactic checks a browser can make truthfully. Each returns a message or
 * `undefined`; none of them decides whether native would accept the value. */
export const syntactic = {
  required: (value: unknown) => (value === undefined || value === null || String(value).trim() === '') ? message('settings:validation.required') : undefined,
  url: (value: unknown) => { if (!value) return undefined; try { new URL(String(value)); return undefined; } catch { return message('settings:validation.url'); } },
  positive: (value: unknown) => value === undefined || value === null || value === '' ? undefined : Number.isFinite(Number(value)) && Number(value) > 0 ? undefined : message('settings:validation.positive'),
};

export function Text<T>({ form, name, label, required = false, url = false, placeholder, validate: additional }: {
  form: TypedUnitForm<T>; name: FieldPath<T>; label: string; required?: boolean; url?: boolean; placeholder?: string;
  validate?: (value: unknown) => DisplayText | undefined;
}) {
  const validate = ({ value }: { value: unknown }) => (required ? syntactic.required(value) : undefined) ?? (url ? syntactic.url(value) : undefined) ?? additional?.(value);
  const Field = form.Field as unknown as (props: { name: string; validators: { onChange: typeof validate }; children: (field: BoundField) => ReactNode }) => ReactNode;
  return <Field name={name as string} validators={{ onChange: validate }}>{field => <label>{label}
    <input aria-label={label} value={(field.state.value as string | null | undefined) ?? ''} placeholder={placeholder} required={required}
      aria-invalid={field.state.meta.errors.filter(Boolean).length > 0 || undefined}
      onBlur={field.handleBlur} onChange={event => field.handleChange(event.target.value as never)} />
    <Errors field={field} label={label}/>
  </label>}</Field>;
}

export function Numeric<T>({ form, name, label, min = 1, empty = null, required = false, validate: additional, listenTo }: {
  form: TypedUnitForm<T>; name: FieldPath<T>; label: string; min?: number; empty?: null | undefined; required?: boolean;
  validate?: (value: unknown) => DisplayText | undefined;
  listenTo?: FieldPath<T>[];
}) {
  const validate = ({ value }: { value: unknown }) => (required ? syntactic.required(value) : undefined) ?? syntactic.positive(value) ?? additional?.(value);
  const Field = form.Field as unknown as (props: { name: string; validators: { onChange: typeof validate; onChangeListenTo?: string[] }; children: (field: BoundField) => ReactNode }) => ReactNode;
  return <Field name={name as string} validators={{ onChange: validate, onChangeListenTo: listenTo }}>{field => <label>{label}
    <input aria-label={label} type="number" min={min} required={required} value={(field.state.value as number | string | null | undefined) ?? ''}
      aria-invalid={field.state.meta.errors.filter(Boolean).length > 0 || undefined}
      onBlur={field.handleBlur} onChange={event => field.handleChange((event.target.value ? Number(event.target.value) : empty) as never)} />
    <Errors field={field} label={label}/>
  </label>}</Field>;
}

/** A numeric field native models as a decimal string, kept as a string so no
 * precision is lost in the browser. */
export function NumericText<T>({ form, name, label, required = false }: { form: TypedUnitForm<T>; name: FieldPath<T>; label: string; required?: boolean }) {
  const validate = ({ value }: { value: unknown }) => {
    if (required && syntactic.required(value)) return syntactic.required(value);
    if (value === undefined || value === null || value === '') return undefined;
    return /^[1-9][0-9]*$/.test(String(value)) && BigInt(String(value)) <= 18446744073709551615n
      ? undefined : message('settings:validation.positive-integer');
  };
  const Field = form.Field as unknown as (props: { name: string; validators: { onChange: typeof validate }; children: (field: BoundField) => ReactNode }) => ReactNode;
  return <Field name={name as string} validators={{ onChange: validate }}>{field => <label>{label}
    <input aria-label={label} type="number" min={1} required={required} value={(field.state.value as string | null | undefined) ?? ''}
      aria-invalid={field.state.meta.errors.filter(Boolean).length > 0 || undefined}
      onBlur={field.handleBlur} onChange={event => field.handleChange((event.target.value || null) as never)} />
    <Errors field={field} label={label}/>
  </label>}</Field>;
}

export function Bool<T>({ form, name, label }: { form: TypedUnitForm<T>; name: FieldPath<T>; label: string }) {
  return <Bound<T, boolean | null | undefined> form={form} name={name}>{(value, change) =>
    <Toggle label={label} checked={!!value} onChange={change} />}</Bound>;
}

/** Provider-native request parameters bound to one form path. */
export function RequestParameters<T>({ form, name, label, optional = true }: {
  form: TypedUnitForm<T>; name: FieldPath<T>; label: string; optional?: boolean;
}) {
  return <Bound<T, RequestParams | null | undefined> form={form} name={name}>{(raw, change) =>
    <JsonObjectEditor label={label} value={raw ?? undefined} optional={optional} change={change} />}</Bound>;
}

/** One native configuration or request-parameter object, edited as whole JSON:
 * paste or type nested objects, arrays and `null` without losing their shape.
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
export function JsonObjectEditor({ label, value, change, optional = true, hint }: {
  label: string; value: RequestParams | undefined; change: (value: RequestParams | undefined) => void; optional?: boolean;
  hint?: string;
}) {
  const tx = useTranslation();
  const display = (current: RequestParams | undefined) => current === undefined ? '' : formatRequestParams(current);
  const [text, setText] = useState(() => display(value));
  const [error, setError] = useNotice();
  const transient = useSettingsFeedback();
  const reflected = useRef(value);
  // Unparsed text is editor-local and never a draft, so the transaction owner
  // cannot reset it through the value alone: a discard or reviewed revision
  // may present the very value this editor already reflects. The owner's
  // reset generation resynchronizes it; any other render keeps the text.
  const resets = useContext(UnitReset);
  const seen = useRef(resets);
  useLayoutEffect(() => {
    const reset = seen.current !== resets;
    seen.current = resets;
    if (!reset && sameJson(value, reflected.current)) return;
    reflected.current = value;
    setText(display(value));
    setError('');
  }, [value, resets]);
  const accept = (next: RequestParams | undefined) => {
    setError('');
    if (sameJson(next, reflected.current)) return;
    reflected.current = next;
    change(next);
  };
  return <div>
    <SettingsNotice text={transient && error ? `${label} · ${error}` : undefined}/>
    <JsonCodeEditor label={label} value={text} error={error} hideErrorText={transient}
      placeholder={tx(optional ? 'settings:fields.request-params-placeholder-optional' : 'settings:fields.request-params-placeholder-required')}
      onChange={next => {
        setText(next);
        if (next.trim() === '') { accept(optional ? undefined : {}); return; }
        const parsed = parseRequestParams(next);
        if (parsed.ok) { accept(parsed.value); return; }
        const failure = parsed.error;
        setError(failure.kind === 'syntax' ? message('settings:fields.request-params-invalid-json')
          : failure.kind === 'not_object' ? message('settings:fields.request-params-not-object')
            : message(failure.kind === 'duplicate_key' ? 'settings:fields.request-params-duplicate-key' : 'settings:fields.request-params-inexact-number',
              { p0: failure.line, p1: failure.column }));
      }} />
    <p className={css.hint}>{hint ?? tx('settings:fields.request-params-hint')}</p>
  </div>;
}
