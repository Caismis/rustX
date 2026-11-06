import { act } from '@testing-library/react';
import { EditorView } from '@uiw/react-codemirror';

function view(element: HTMLElement) {
  const editor = EditorView.findFromDOM(element);
  if (!editor) throw new Error('Expected a mounted CodeMirror editor');
  return editor;
}
export const readJson = (element: HTMLElement) => view(element).state.doc.toString();
export function changeJson(element: HTMLElement, text: string) {
  act(() => { const editor = view(element); editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: text } }); });
}
export function jsonValidity(element: HTMLElement) {
  const control = element.closest('[data-json-editor]')?.querySelector('input');
  if (!control) throw new Error('Expected the editor constraint-validation control');
  return control;
}
