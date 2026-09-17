import { useState } from 'react';
import { Modal } from '../src/presentation/primitives/Modal';
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { Menu } from '../src/presentation/primitives/Menu';
import { DisclosureRow } from '../src/presentation/primitives/DisclosureRow';
afterEach(cleanup);
it('Harness Menu never dispatches a disabled entry', () => {
  const select = vi.fn();
  const view = render(<Menu open anchor={<button>Actions</button>} items={[{ id: 'a', label: 'A', disabled: true }]} onClose={() => {}} onSelect={select} />);
  fireEvent.click(view.getByRole('menuitem', { name: 'A' }));
  expect(select).not.toHaveBeenCalled();
});
it('icon-only disclosure has an accessible name and a controlled content target', () => {
  const toggle = vi.fn();
  const view = render(<DisclosureRow icon={null} title="Details" open={false} expandable onToggle={toggle}>Body</DisclosureRow>);
  const button = view.getByRole('button', { name: 'Details' });
  const target = document.getElementById(button.getAttribute('aria-controls')!);
  expect(target?.hidden).toBe(true); fireEvent.click(button); expect(toggle).toHaveBeenCalledOnce();
});

it('closing a nested modal keeps its parent isolated and Escape settles only one presentation layer', () => {
  const root = document.createElement('div'); root.id = 'root'; document.body.append(root);
  function Nested() {
    const [outer, setOuter] = useState(true), [inner, setInner] = useState(false);
    return <><Modal open={outer} title="Outer" closeLabel="Close outer" onClose={() => setOuter(false)}><button onClick={() => setInner(true)}>Open inner</button></Modal>
      <Modal open={inner} title="Inner" closeLabel="Close inner" onClose={() => setInner(false)}><button>Inside</button></Modal></>;
  }
  const view = render(<Nested />, { container: root });
  expect(root.inert).toBe(true);
  fireEvent.click(screen.getByRole('button', { name: 'Open inner' }));
  fireEvent.keyDown(document, { key: 'Escape' });
  expect(screen.queryByRole('dialog', { name: 'Inner' })).toBeNull();
  expect(screen.getByRole('dialog', { name: 'Outer' })).toBeTruthy();
  expect(root.inert).toBe(true);
  fireEvent.keyDown(document, { key: 'Escape' });
  expect(root.inert).toBe(false);
  view.unmount(); root.remove();
});
