import { useEffect, useRef, useState } from 'react';
import { Button } from '../../presentation/primitives/Button';
/** Provider request overlays only. These are explicit drafts, never effective defaults. */
export function RequestPolicy<T extends Record<string, unknown>>({ value, change }: { value: T; change: (value: T) => void }) {
  const [key, setKey] = useState('');
  return <fieldset><legend>Explicit request parameters</legend>{Object.entries(value).map(([name, parameter]) => <div key={name}><label>{name}<Parameter value={parameter} change={next => change({ ...value, [name]: next })} /></label><Button onClick={() => { const next = { ...value }; delete next[name]; change(next); }}>Remove {name}</Button></div>)}<label>Parameter name<input value={key} onChange={e => setKey(e.target.value)} /></label><Button disabled={!key || key in value} onClick={() => { change({ ...value, [key]: '' }); setKey(''); }}>Add parameter</Button><p>Each value is a JSON scalar, array, or object. Native Rust validates protected keys and protocol support. Do not enter secrets.</p></fieldset>;
}
function Parameter({ value, change }: { value: unknown; change: (value: unknown) => void }) {
  const [text, setText] = useState(JSON.stringify(value) ?? ''), [error, setError] = useState('');
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => { setText(JSON.stringify(value) ?? ''); setError(''); input.current?.setCustomValidity(''); }, [value]);
  return <><input ref={input} value={text} aria-invalid={!!error} onChange={e => { const text = e.target.value; setText(text); try { const next = JSON.parse(text); change(next); e.target.setCustomValidity(''); setError(''); } catch { e.target.setCustomValidity('Enter a complete JSON value before saving.'); setError('Enter a complete JSON value before saving.'); } }} />{error && <span role="alert">{error}</span>}</>;
}
