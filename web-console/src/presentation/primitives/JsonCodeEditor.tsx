import { lazy, Suspense, useCallback, useId, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { EditorView } from '@uiw/react-codemirror';
import { useTranslation } from '../../locale/react';
import { Button } from './Button';
import { formatJsonCode, jsonSyntaxErrors } from './json-code';
import css from './JsonCodeEditor.module.css';

const CodeView = lazy(() => import('./JsonCodeView'));

/** One settings JSON surface. The native constraint-validation control makes
 * contenteditable participate in its enclosing form and disabled fieldsets.
 * It carries no authored value; invalid submission focuses the actual editor. */
export function JsonCodeEditor({ label, value, onChange, error = '', allowEmpty = true, placeholder, minHeight = 144 }: {
  label: string; value: string; onChange: (text: string) => void; error?: string;
  allowEmpty?: boolean; placeholder?: string; minHeight?: number;
}) {
  const tx = useTranslation(), id = useId();
  const editor = useRef<EditorView | undefined>(undefined);
  const control = useRef<HTMLInputElement>(null);
  const [disabled, setDisabled] = useState(false);
  const syntax = useMemo(() => jsonSyntaxErrors(value, allowEmpty), [value, allowEmpty]);
  const diagnostics = useMemo(() => syntax.map(item => ({ ...item, severity: 'error' as const,
    message: tx('settings:json-editor.syntax-error', { p0: item.line, p1: item.column }),
  })), [syntax, tx]);
  const problem = error || diagnostics[0]?.message || '';
  useLayoutEffect(() => { control.current?.setCustomValidity(problem); }, [problem]);
  useLayoutEffect(() => {
    const input = control.current!;
    const update = () => setDisabled(input.matches(':disabled'));
    const observer = new MutationObserver(update);
    for (let parent = input.parentElement; parent; parent = parent.parentElement) {
      if (parent instanceof HTMLFieldSetElement) observer.observe(parent, { attributes: true, attributeFilter: ['disabled'] });
    }
    update();
    return () => observer.disconnect();
  }, []);
  const formatDocument = useCallback(() => {
    if (disabled) return false;
    const view = editor.current;
    if (!view || jsonSyntaxErrors(view.state.doc.toString(), allowEmpty).length) return false;
    const text = view.state.doc.toString(), formatted = formatJsonCode(text);
    if (formatted !== text) view.dispatch({ changes: { from: 0, to: text.length, insert: formatted } });
    return true;
  }, [disabled, allowEmpty]);
  return <div className={css.root} data-json-editor="" data-empty={!value.trim()} data-invalid={!!problem} data-disabled={disabled}>
    <div className={css.header}>
      <label id={`${id}-label`} onClick={() => editor.current?.focus()}>{label}</label>
      <Button size="sm" disabled={disabled || !value.trim() || !!syntax.length} onClick={formatDocument}
        aria-label={tx('settings:json-editor.format-label', { p0: label })} title={tx('settings:json-editor.format-shortcut')}>
        {tx('settings:json-editor.format')}
      </Button>
    </div>
    <input ref={control} className={css.validity} aria-hidden="true" tabIndex={-1} value="" onChange={() => {}}
      onInvalid={event => { event.preventDefault(); editor.current?.focus(); }} />
    <Suspense fallback={<div className={css.loading} data-json-editor-loading="" role="status">{tx('settings:json-editor.loading')}</div>}>
      <CodeView labelId={`${id}-label`} descriptionId={`${id}-status`} value={value} onChange={onChange}
        editor={editor} diagnostics={diagnostics} invalid={!!problem} disabled={disabled}
        placeholder={placeholder} minHeight={minHeight} formatDocument={formatDocument} />
    </Suspense>
    <div id={`${id}-status`} className={css.status}>
      {problem ? <>
        <span role="alert">{problem}</span>
        {error && diagnostics[0] && <small>{diagnostics[0].message}</small>}
      </> : <small>{tx('settings:json-editor.auto-format')}</small>}
    </div>
  </div>;
}
