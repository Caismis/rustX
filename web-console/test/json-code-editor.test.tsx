import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { useState } from 'react';
import { JsonCodeEditor } from '../src/presentation/primitives/JsonCodeEditor';
import { formatJsonCode, jsonSyntaxErrors } from '../src/presentation/primitives/json-code';
import { changeJson, jsonValidity, readJson } from './json-editor-helpers';
afterEach(cleanup);

function Example({ disabled = false, value = '{"temperature":0.3}', changed = () => {} }: { disabled?: boolean; value?: string; changed?: (text: string) => void }) {
  const [text, setText] = useState(value);
  return <form aria-label="Example"><fieldset disabled={disabled}>
    <JsonCodeEditor label="Parameters" value={text} onChange={next => { setText(next); changed(next); }} />
    <button type="button">Leave editor</button>
  </fieldset></form>;
}

it('marks strict syntax errors with line/column, preserves text and blocks native form submission', async () => {
  render(<Example />);
  const editor = await screen.findByRole('textbox', { name: 'Parameters' });
  changeJson(editor, '{\n  "secret-key":\n}');
  expect(readJson(editor)).toBe('{\n  "secret-key":\n}');
  expect(screen.getAllByRole('alert')[0].textContent).toMatch(/line 3, column 1/);
  expect(screen.getAllByRole('alert').map(node => node.textContent).join('')).not.toContain('secret-key');
  expect((screen.getByRole('form') as HTMLFormElement).checkValidity()).toBe(false);
  expect(document.querySelector('.cm-lintRange-error')).toBeTruthy();
  expect((screen.getByRole('button', { name: 'Format JSON: Parameters' }) as HTMLButtonElement).disabled).toBe(true);
  changeJson(editor, '{"valid": true}');
  expect(jsonValidity(editor).validity.valid).toBe(true);
  expect(screen.queryByRole('alert')).toBeNull();
  expect((screen.getByRole('form') as HTMLFormElement).checkValidity()).toBe(true);
});

it('formats on leaving, provides a format action and shortcut, and keeps formatting undoable', async () => {
  render(<Example />);
  const editor = await screen.findByRole('textbox', { name: 'Parameters' });
  expect(document.querySelector('.cm-lineNumbers')).toBeTruthy();
  expect(document.querySelector('.tok-number')).toBeTruthy();
  fireEvent.blur(editor, { relatedTarget: screen.getByRole('button', { name: 'Leave editor' }) });
  expect(readJson(editor)).toBe('{\n  "temperature": 0.3\n}');
  fireEvent.keyDown(editor, { key: 'z', code: 'KeyZ', ctrlKey: true });
  expect(readJson(editor)).toBe('{"temperature":0.3}');
  fireEvent.click(screen.getByRole('button', { name: 'Format JSON: Parameters' }));
  expect(readJson(editor)).toBe('{\n  "temperature": 0.3\n}');
  changeJson(editor, '{"a":[1,true,null]}');
  fireEvent.keyDown(editor, { key: 'F', code: 'KeyF', keyCode: 70, which: 70, ctrlKey: true, shiftKey: true });
  expect(readJson(editor)).toContain('\n  "a": [\n    1,');
});

it('follows external reset without emitting it and retains the mounted editing surface', async () => {
  const changed = vi.fn();
  const view = render(<JsonCodeEditor label="Parameters" value="{" onChange={changed} />);
  const editor = await screen.findByRole('textbox', { name: 'Parameters' });
  expect(jsonValidity(editor).validity.valid).toBe(false);
  view.rerender(<JsonCodeEditor label="Parameters" value={'{"reset":true}'} onChange={changed} />);
  await waitFor(() => expect(readJson(editor)).toBe('{"reset":true}'));
  expect(screen.getByRole('textbox', { name: 'Parameters' })).toBe(editor);
  expect(jsonValidity(editor).validity.valid).toBe(true);
  expect(screen.queryByRole('alert')).toBeNull();
  view.rerender(<JsonCodeEditor label="Parameters" value="" onChange={changed} />);
  await waitFor(() => expect(document.querySelector('.cm-gutters')).toBeNull());
  expect(screen.getByRole('textbox', { name: 'Parameters' })).toBe(editor);
  expect(readJson(editor)).toBe('');
  view.rerender(<JsonCodeEditor label="Parameters" value={'\n  '} onChange={changed} />);
  await waitFor(() => expect(readJson(editor)).toBe('\n  '));
  expect(document.querySelector('.cm-gutters')).toBeNull();
  view.rerender(<JsonCodeEditor label="Parameters" value={'{"restored":true}'} onChange={changed} />);
  await waitFor(() => expect(document.querySelector('.cm-lineNumbers')).toBeTruthy());
  expect(screen.getByRole('textbox', { name: 'Parameters' })).toBe(editor);
  expect(changed).not.toHaveBeenCalled();
});

it('inherits disabled fieldsets initially and after an authority change', async () => {
  const view = render(<Example disabled />);
  const editor = await screen.findByRole('textbox', { name: 'Parameters' });
  expect(editor.getAttribute('contenteditable')).toBe('false');
  expect(editor.getAttribute('aria-disabled')).toBe('true');
  expect(jsonValidity(editor).matches(':disabled')).toBe(true);
  view.rerender(<Example />);
  await waitFor(() => expect(editor.getAttribute('contenteditable')).toBe('true'));
  changeJson(editor, '{');
  expect((screen.getByRole('form') as HTMLFormElement).checkValidity()).toBe(false);
  view.rerender(<Example disabled />);
  await waitFor(() => expect(editor.getAttribute('contenteditable')).toBe('false'));
  expect((screen.getByRole('form') as HTMLFormElement).checkValidity()).toBe(true);
  await act(async () => {});
});

it('only changes whitespace, keeping repeated keys, escaped spelling and exact numeric tokens', () => {
  const text = '{"seed":9007199254740993,"seed":1.230000,"escaped":"\\u0061","items":[null,true]}';
  const formatted = formatJsonCode(text);
  expect(formatted).toContain('"seed": 9007199254740993,\n  "seed": 1.230000,');
  expect(formatted).toContain('"escaped": "\\u0061"');
  expect(jsonSyntaxErrors(formatted)).toEqual([]);
  expect(formatJsonCode(formatted)).toBe(formatted);
});

it.each(['{"value":}', '{"a":1,}', '{/* comment */"a":1}', '{"a":NaN}', '{"a":01}'])('never formats invalid strict JSON %s', text => {
  expect(jsonSyntaxErrors(text).length).toBeGreaterThan(0);
  expect(formatJsonCode(text)).toBe(text);
});
