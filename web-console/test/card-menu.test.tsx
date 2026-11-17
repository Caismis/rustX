import { useState } from 'react';
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { CardMenu } from '../src/presentation/catalog/CardMenu';

afterEach(cleanup);

function Harness({ disabled }: { disabled: boolean }) {
  const [selected, setSelected] = useState<string[]>([]);
  return (
    <>
      <CardMenu
        label="Actions for global-guide"
        items={[
          { id: 'edit', label: 'Edit display information' },
          { id: 'move-scope', label: 'Move to Workspace', disabled },
        ]}
        onSelect={id => setSelected(current => [...current, id])}
      />
      <output data-testid="selected">{selected.join(',')}</output>
    </>
  );
}

function selected() {
  return screen.getByTestId('selected').textContent;
}

function openMenu() {
  fireEvent.click(screen.getByRole('button', { name: 'Actions for global-guide' }));
  return screen.getByRole('menuitem', { name: 'Move to Workspace' }) as HTMLButtonElement;
}

it('keeps the opened row clickable when a refresh disables the replacement row', () => {
  const ui = render(<Harness disabled={false} />);
  const item = openMenu();
  ui.rerender(<Harness disabled={true} />);
  const kept = screen.getByRole('menuitem', { name: 'Move to Workspace' }) as HTMLButtonElement;
  expect(kept).toBe(item);
  expect(kept.disabled).toBe(false);
  fireEvent.click(kept);
  expect(selected()).toBe('move-scope');
  expect(screen.queryByRole('menuitem')).toBeNull();
});

it('does not select when the pointer is released outside the row', () => {
  render(<Harness disabled={false} />);
  const item = openMenu();
  fireEvent.pointerDown(item, { button: 0 });
  fireEvent.pointerUp(document.body, { button: 0 });
  expect(selected()).toBe('');
  expect(screen.getByRole('menuitem', { name: 'Move to Workspace' })).toBeTruthy();
});

it('closes on Escape and selects once from a keyboard click', () => {
  render(<Harness disabled={false} />);
  openMenu();
  fireEvent.keyDown(document, { key: 'Escape' });
  expect(screen.queryByRole('menuitem')).toBeNull();
  expect(selected()).toBe('');
  fireEvent.click(openMenu(), { detail: 0 });
  expect(selected()).toBe('move-scope');
  expect(screen.queryByRole('menuitem')).toBeNull();
});
