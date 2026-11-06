import { useLayoutEffect, useMemo, type RefObject } from 'react';
import CodeMirror, { EditorView, ExternalChange, keymap } from '@uiw/react-codemirror';
import { json } from '@codemirror/lang-json';
import { syntaxHighlighting } from '@codemirror/language';
import { classHighlighter } from '@lezer/highlight';
import { linter, lintGutter, setDiagnostics, type Diagnostic } from '@codemirror/lint';

const basicSetup = { syntaxHighlighting: false, highlightActiveLine: false, highlightSelectionMatches: false, autocompletion: false };
const emptySetup = { ...basicSetup, lineNumbers: false, foldGutter: false, highlightActiveLineGutter: false };
const panelSetup = { ...basicSetup, foldGutter: false, highlightActiveLineGutter: false };
const jsonExtensions = [json(), syntaxHighlighting(classHighlighter), EditorView.lineWrapping];
const diagnosticsGutter = lintGutter();

/** Lazy CodeMirror engine; its document, history and selection survive renders. */
export default function JsonCodeView({ labelId, descriptionId, value, onChange, editor, diagnostics, invalid, disabled, placeholder, minHeight, fill = false, formatDocument }: {
  labelId: string; descriptionId: string; value: string; onChange: (text: string) => void;
  editor: RefObject<EditorView | undefined>; diagnostics: Diagnostic[]; invalid: boolean; disabled: boolean;
  placeholder?: string; minHeight: number; fill?: boolean; formatDocument: () => boolean;
}) {
  const empty = !value.trim();
  const extensions = useMemo(() => [...jsonExtensions,
    ...(empty ? [] : [diagnosticsGutter]),
    linter(() => diagnostics, { delay: 0 }),
    EditorView.contentAttributes.of({ 'aria-labelledby': labelId, 'aria-describedby': descriptionId, 'aria-invalid': invalid ? 'true' : 'false', 'aria-disabled': String(disabled), spellcheck: 'false' }),
    keymap.of([{ key: 'Mod-Shift-f', run: formatDocument }, { key: 'Alt-Shift-f', run: formatDocument }]),
  ], [empty, labelId, descriptionId, invalid, disabled, formatDocument, diagnostics]);
  useLayoutEffect(() => {
    const view = editor.current;
    if (!view) return;
    // Owner resets must replace an unparsed buffer immediately. The React
    // wrapper normally defers external values while its typing latch is open.
    if (view.state.doc.toString() !== value) view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: value }, annotations: ExternalChange.of(true),
    });
    view.dispatch(setDiagnostics(view.state, diagnostics));
  }, [value, diagnostics]);
  useLayoutEffect(() => () => { editor.current = undefined; }, [editor]);
  return <CodeMirror value={value} onChange={onChange} theme="none" basicSetup={empty ? emptySetup : fill ? panelSetup : basicSetup}
    height={fill ? '100%' : undefined} minHeight={`${minHeight}px`} maxHeight={fill ? undefined : '360px'} extensions={extensions} placeholder={placeholder}
    editable={!disabled} readOnly={disabled} indentWithTab={false}
    onCreateEditor={view => { editor.current = view; view.dispatch(setDiagnostics(view.state, diagnostics)); }}
    onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) formatDocument(); }} />;
}
