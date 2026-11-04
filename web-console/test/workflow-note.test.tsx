import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import type { WorkflowNote } from '../../protocol/app-server/v68';
import { NoteActions, NoteNode } from '../src/app/workflows/ui/editor/note/NoteNode';

afterEach(cleanup);

// A Lexical state as Dify's note stores it: "Hello" with "world" in bold.
const text = JSON.stringify({ root: { type: 'root', version: 1, direction: null, format: '', indent: 0, children: [{
  type: 'paragraph', version: 1, direction: null, format: '', indent: 0, textFormat: 0, textStyle: '', children: [
    { type: 'text', version: 1, detail: 0, format: 0, mode: 'normal', style: '', text: 'Hello ' },
    { type: 'text', version: 1, detail: 0, format: 1, mode: 'normal', style: '', text: 'world' },
  ] }] } });
const note: WorkflowNote = { id: 'note', block: [], text, theme: 'blue', position: { x: 0, y: 0 }, dimensions: { width: 240, height: 88 } };

function mount(selected: boolean) {
  const actions = { editable: true, change: vi.fn(), copy: vi.fn(), duplicate: vi.fn(), remove: vi.fn() };
  render(<NoteActions.Provider value={actions}><NoteNode id="view:note" note={note} selected={selected} resizer={null} /></NoteActions.Provider>);
  return actions;
}

it('writes a note in place from its stored rich text, with no toolbar until selected', async () => {
  mount(false);
  await act(async () => {});
  const editor = screen.getByRole('textbox', { name: 'Note' });
  expect(editor.getAttribute('contenteditable')).toBe('true');
  expect(editor.textContent).toBe('Hello world');
  expect(editor.querySelector('strong, b, [class*="bold"]')?.textContent).toBe('world');
  expect(screen.queryByRole('toolbar')).toBeNull();
});

it('shows Dify\'s toolbar when selected: color, font size, marks, link, list and more', async () => {
  const actions = mount(true);
  await act(async () => {});
  const toolbar = screen.getByRole('toolbar', { name: 'Note' });
  for (const name of ['Note color', 'Font size', 'Bold', 'Italic', 'Strikethrough', 'Link', 'Bullet list', 'More']) expect(toolbar.querySelector(`[aria-label="${name}"]`)).not.toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Note color' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Green' }));
  expect(actions.change).toHaveBeenCalledWith('view:note', { theme: 'green' });
});

it('reads text that is not a Lexical state as one plain paragraph, unchanged until edited', async () => {
  const actions = { editable: true, change: vi.fn(), copy: vi.fn(), duplicate: vi.fn(), remove: vi.fn() };
  render(<NoteActions.Provider value={actions}><NoteNode id="view:note" note={{ ...note, text: 'Keep relative' }} selected={false} resizer={null} /></NoteActions.Provider>);
  await act(async () => {});
  expect(screen.getByRole('textbox', { name: 'Note' }).textContent).toBe('Keep relative');
  expect(actions.change).not.toHaveBeenCalled();
});
