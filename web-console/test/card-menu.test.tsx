import { useState } from 'react';
import { afterEach, expect, it } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { CardMenu } from '../src/presentation/catalog/CardMenu';

afterEach(cleanup);

function Harness({ disabled, token }: { disabled: boolean; token: string }) {
  const [selected, setSelected] = useState<string[]>([]);
  return (
    <>
      <CardMenu
        label="Actions for global-guide"
        items={[
          { id: 'edit', label: 'Edit display information' },
          { id: 'move-scope', label: 'Move to Workspace', disabled },
        ]}
        onSelect={id => setSelected(current => [...current, `${token}:${id}`])}
      />
      <output data-testid="selected">{selected.join(',')}</output>
    </>
  );
}

function selected() {
  return screen.getByTestId('selected').textContent;
}

const press = { button: 0, pointerId: 1, isPrimary: true };

function openMenu() {
  fireEvent.click(screen.getByRole('button', { name: 'Actions for global-guide' }));
  return screen.getByRole('menuitem', { name: 'Move to Workspace' }) as HTMLButtonElement;
}

function finishPress(row: HTMLButtonElement) {
  fireEvent.pointerUp(row, press);
  fireEvent.click(row);
}

it('keeps the pressed row and its selection closure through pointerdown, pointerup and click', () => {
  const ui = render(<Harness disabled={false} token="opened" />);
  const item = openMenu();
  fireEvent.pointerDown(item, press);
  fireEvent.pointerDown(item, { button: 0, pointerId: 2, isPrimary: false });
  ui.rerender(<Harness disabled={true} token="replacement" />);
  const kept = screen.getByRole('menuitem', { name: 'Move to Workspace' }) as HTMLButtonElement;
  expect(kept).toBe(item);
  expect(kept.disabled).toBe(false);
  finishPress(kept);
  expect(selected()).toBe('opened:move-scope');
  expect(screen.queryByRole('menuitem')).toBeNull();
});

it('enables an open action and uses the settled closure when a pending catalog refresh finishes', () => {
  const ui = render(<Harness disabled={true} token="pending" />);
  const item = openMenu();
  expect(item.disabled).toBe(true);
  ui.rerender(<Harness disabled={false} token="settled" />);
  const enabled = screen.getByRole('menuitem', { name: 'Move to Workspace' }) as HTMLButtonElement;
  expect(enabled.disabled).toBe(false);
  fireEvent.pointerDown(enabled, press);
  fireEvent.pointerUp(enabled, press);
  fireEvent.click(enabled);
  expect(selected()).toBe('settled:move-scope');
  expect(screen.queryByRole('menuitem')).toBeNull();
});

it('follows a settled disable while the menu is open and no pointer gesture is active', () => {
  const ui = render(<Harness disabled={false} token="opened" />);
  openMenu();
  ui.rerender(<Harness disabled={true} token="settled" />);
  expect((screen.getByRole('menuitem', { name: 'Move to Workspace' }) as HTMLButtonElement).disabled).toBe(true);
});

it('does not keep a disabled press or a cancelled press', () => {
  const ui = render(<Harness disabled={true} token="pending" />);
  const disabled = openMenu();
  fireEvent.pointerDown(disabled, press);
  fireEvent.pointerUp(disabled, press);
  fireEvent.click(disabled);
  expect(selected()).toBe('');
  ui.rerender(<Harness disabled={false} token="settled" />);
  const enabled = screen.getByRole('menuitem', { name: 'Move to Workspace' }) as HTMLButtonElement;
  fireEvent.pointerDown(enabled, press);
  ui.rerender(<Harness disabled={true} token="cancelled" />);
  expect(enabled.disabled).toBe(false);
  fireEvent.pointerCancel(enabled, press);
  expect(enabled.disabled).toBe(true);
  expect(selected()).toBe('');
});

it('drops the held row when the pointer is released on another row', () => {
  const ui = render(<Harness disabled={false} token="opened" />);
  openMenu();
  const copy = screen.getByRole('menuitem', { name: 'Edit display information' }) as HTMLButtonElement;
  const move = screen.getByRole('menuitem', { name: 'Move to Workspace' }) as HTMLButtonElement;
  fireEvent.pointerDown(move, press);
  ui.rerender(<Harness disabled={true} token="replacement" />);
  expect(move.disabled).toBe(false);
  fireEvent.pointerUp(copy, press);
  expect(selected()).toBe('');
  expect(move.disabled).toBe(true);
});

it('releases the held row when the pointer is released outside after a refresh disables it', () => {
  const ui = render(<Harness disabled={false} token="opened" />);
  const item = openMenu();
  fireEvent.pointerDown(item, press);
  ui.rerender(<Harness disabled={true} token="replacement" />);
  expect(item.disabled).toBe(false);
  fireEvent.pointerUp(document.body, press);
  expect(item.disabled).toBe(true);
  expect(selected()).toBe('');
  fireEvent(item, new MouseEvent('click', { bubbles: true, cancelable: true, detail: 1 }));
  expect(selected()).toBe('');
});

it('closes on Escape and selects once from a keyboard click', () => {
  render(<Harness disabled={false} token="opened" />);
  openMenu();
  fireEvent.keyDown(document, { key: 'Escape' });
  expect(screen.queryByRole('menuitem')).toBeNull();
  expect(selected()).toBe('');
  fireEvent.click(openMenu(), { detail: 0 });
  expect(selected()).toBe('opened:move-scope');
  expect(screen.queryByRole('menuitem')).toBeNull();
});
