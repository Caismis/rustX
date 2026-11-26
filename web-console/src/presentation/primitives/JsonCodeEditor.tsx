import { lazy, Suspense, useCallback, useId, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { EditorView } from '@uiw/react-codemirror';
import { useTranslation } from '../../locale/react';
import { Button } from './Button';
import { formatJsonCode, jsonSyntaxErrors } from './json-code';
import { IconWrapLinesOutline16 } from './icons';

const CodeView = lazy(() => import('./JsonCodeView'));

/** One settings JSON surface. The native constraint-validation control makes
 * contenteditable participate in its enclosing form and disabled fieldsets.
 * It carries no authored value; invalid submission focuses the actual editor. */
export function JsonCodeEditor({ label, value, onChange, error = '', hideErrorText = false, allowEmpty = true, placeholder, minHeight = 144, panel = false, headerActions }: {
  label: string; value: string; onChange: (text: string) => void; error?: string; hideErrorText?: boolean;
  allowEmpty?: boolean; placeholder?: string; minHeight?: number; panel?: boolean; headerActions?: ReactNode;
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
  return <div className="min-w-0 relative my-[8px] [&_.cm-editor]:overflow-hidden [&_.cm-editor]:[border:1px_solid_var(--dsw-alias-border-l4)] [&_.cm-editor]:rounded-[8px] [&_.cm-editor]:bg-[var(--dsw-alias-bg-layer-3)] [&_.cm-editor]:[color:var(--dsw-alias-label-primary)] [&_.cm-focused]:[outline:2px_solid_var(--dsw-alias-state-business-primary)] [&_.cm-focused]:outline-offset-[-1px] [&_.cm-scroller]:[font:13px/21px_var(--ds-font-family-code),monospace] [&_.cm-scroller]:overscroll-contain [&_.cm-content]:py-[10px] [&_.cm-content]:[caret-color:var(--dsw-alias-label-primary)] [&_.cm-line]:px-[8px] [&_.cm-gutters]:bg-[var(--dsw-alias-bg-layer-2)] [&_.cm-gutters]:[color:var(--dsw-alias-label-secondary)] [&_.cm-gutters]:[border-right:1px_solid_var(--dsw-alias-border-l3)] [&_.cm-gutterElement]:px-[6px] [&_.cm-activeLineGutter]:bg-[var(--dsw-alias-interactive-bg-hover)] [&_.cm-selectionBackground]:bg-[var(--dsw-alias-interactive-bg-hover)] [&_.cm-cursor]:[border-left-color:var(--dsw-alias-label-primary)] [&_.cm-placeholder]:[color:var(--dsw-alias-label-tertiary)] [&_.cm-tooltip]:z-[1300] [&_.cm-tooltip]:bg-[var(--dsw-alias-bg-layer-2)] [&_.cm-tooltip]:[color:var(--dsw-alias-label-primary)] [&_.cm-tooltip]:[border-color:var(--dsw-alias-border-l3)] [&_.cm-diagnostic]:wrap-anywhere [&_.cm-diagnostic]:whitespace-normal [&_.tok-propertyName]:[color:#881391] [&_.tok-string]:[color:#a31515] [&_.tok-number]:[color:#1c00cf] [&_.tok-bool]:[color:#1c00cf] [&_.tok-null]:[color:#1c00cf] dark:[&_.tok-propertyName]:[color:#5db0d7] dark:[&_.tok-string]:[color:#f28b82] dark:[&_.tok-number]:[color:#99c8ff] dark:[&_.tok-bool]:[color:#99c8ff] dark:[&_.tok-null]:[color:#99c8ff] [&[data-invalid=true]_.cm-editor]:[border-color:var(--dsw-alias-state-error-primary)] [&[data-disabled=true]_.cm-editor]:opacity-50 [&[data-empty=true]_.cm-line]:px-[12px] [&[data-empty=true]_button]:invisible [&[data-invalid=true]_.jsonstatus-hook]:[color:var(--dsw-alias-label-primary)] [&[data-invalid=true]_.jsonstatus-hook]:[border-left:2px_solid_var(--dsw-alias-state-error-primary)] [&[data-invalid=true]_.jsonstatus-hook]:px-[8px] [&[data-invalid=true]_.jsonstatus-hook]:py-[6px] [&[data-invalid=true]_.jsonstatus-hook]:rounded-[6px] [&[data-invalid=true]_.jsonstatus-hook]:bg-[color-mix(in_srgb,var(--dsw-alias-state-error-primary)_8%,transparent)] [&[data-panel=true]]:flex [&[data-panel=true]]:flex-col [&[data-panel=true]]:flex-1 [&[data-panel=true]]:min-h-0 [&[data-panel=true]]:m-0 [&[data-panel=true]]:px-[8px] [&[data-panel=true]]:py-[6px] [&[data-panel=true]]:rounded-[12px] [&[data-panel=true]]:bg-[var(--dsw-alias-interactive-bg-hover-solid)] [&[data-panel=true]]:overflow-hidden [&[data-panel=true]>.cm-theme]:flex-1 [&[data-panel=true]>.cm-theme]:min-h-0 [&[data-panel=true]_.cm-editor]:border-0 [&[data-panel=true]_.cm-editor]:rounded-none [&[data-panel=true]_.cm-editor]:bg-transparent [&[data-panel=true]_.cm-gutters]:bg-transparent [&[data-panel=true]_.cm-gutters]:border-0 [&[data-panel=true]_.cm-content]:pt-0 [&[data-panel=true]_.jsonstatus-hook[data-problem=false]]:absolute [&[data-panel=true]_.jsonstatus-hook[data-problem=false]]:w-[1px] [&[data-panel=true]_.jsonstatus-hook[data-problem=false]]:h-[1px] [&[data-panel=true]_.jsonstatus-hook[data-problem=false]]:overflow-hidden [&[data-panel=true]_.jsonstatus-hook[data-problem=false]]:[clip-path:inset(50%)]" data-json-editor="" data-panel={panel || undefined} data-empty={!value.trim()} data-invalid={!!problem} data-disabled={disabled}>
    <div className="flex items-center justify-between gap-[8px] mb-[6px] [&_label]:p-0 [&_label]:cursor-text [&_label]:min-w-0 [&_label]:wrap-anywhere [&_label]:text-[13px] [&_label]:leading-[20px] [&_label]:[color:var(--dsw-alias-label-secondary)] [&_button]:flex-none [&_button]:text-[12px] [[data-panel=true]>&]:min-h-[22px] [[data-panel=true]>&]:mb-0 [[data-panel=true]>&]:px-[4px] [[data-panel=true]>&_label]:text-[12px] [[data-panel=true]>&_label]:font-medium">
      <label id={`${id}-label`} onClick={() => editor.current?.focus()}>{label}</label>
      <div className="flex items-center gap-[2px] [[data-panel=true]_&_button]:inline-flex [[data-panel=true]_&_button]:items-center [[data-panel=true]_&_button]:justify-center [[data-panel=true]_&_button]:w-[26px] [[data-panel=true]_&_button]:h-[24px] [[data-panel=true]_&_button]:min-h-0 [[data-panel=true]_&_button]:p-[3px] [[data-panel=true]_&_button]:border-0 [[data-panel=true]_&_button]:rounded-[5px] [[data-panel=true]_&_button]:bg-transparent [[data-panel=true]_&_button]:[color:var(--dsw-alias-label-secondary)] [[data-panel=true]_&_button:hover]:bg-[var(--dsw-alias-interactive-bg-hover)]"><Button size="sm" disabled={disabled || !value.trim() || !!syntax.length} onClick={formatDocument}
        aria-label={tx('settings:json-editor.format-label', { p0: label })} title={tx('settings:json-editor.format-shortcut')}>
        {panel ? <IconWrapLinesOutline16 /> : tx('settings:json-editor.format')}
      </Button>{headerActions}</div>
    </div>
    <input ref={control} className="absolute w-[1px]! h-[1px]! min-h-0! p-0! border-0! overflow-hidden [clip-path:inset(50%)]" aria-hidden="true" tabIndex={-1} value="" onChange={() => {}}
      onInvalid={event => { event.preventDefault(); editor.current?.focus(); }} />
    <Suspense fallback={<div className="min-h-[144px] grid place-items-center [color:var(--dsw-alias-label-secondary)] [border:1px_solid_var(--dsw-alias-border-l3)] rounded-[8px] text-[12px]" data-json-editor-loading="" role="status">{tx('settings:json-editor.loading')}</div>}>
      <CodeView labelId={`${id}-label`} descriptionId={`${id}-status`} value={value} onChange={onChange}
        editor={editor} diagnostics={diagnostics} invalid={!!problem} disabled={disabled}
        placeholder={placeholder} minHeight={minHeight} fill={panel} formatDocument={formatDocument} />
    </Suspense>
    <div id={`${id}-status`} className="jsonstatus-hook flex flex-col gap-[2px] mt-[6px] wrap-anywhere text-[12px] leading-[18px] [color:var(--dsw-alias-label-secondary)] [&[hidden]]:hidden [&_small]:text-[11px] [&_small]:leading-[16px] [&_small]:[color:inherit]" hidden={hideErrorText && !!problem} data-problem={!!problem}>
      {hideErrorText && problem ? null : problem ? <>
        <span role="alert">{problem}</span>
        {error && diagnostics[0] && <small>{diagnostics[0].message}</small>}
      </> : <small>{tx('settings:json-editor.auto-format')}</small>}
    </div>
  </div>;
}
