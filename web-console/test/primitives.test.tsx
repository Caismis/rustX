import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render } from '@testing-library/react';
import { Menu } from '../src/presentation/primitives/Menu';
import { Popover } from '../src/presentation/primitives/Popover';
import { DisclosureRow } from '../src/presentation/primitives/DisclosureRow';
afterEach(cleanup);
it('disabled menu and popover cannot open or dispatch an action', () => {
  const select = vi.fn();
  const view = render(<><Menu label="Actions" disabled items={[{ id: 'a', label: 'A' }]} onSelect={select} /><Popover label="Details" disabled>Private panel</Popover></>);
  fireEvent.click(view.getByRole('button', { name: 'Actions' }));
  fireEvent.click(view.getByRole('button', { name: 'Details' }));
  expect(view.queryByRole('menu')).toBeNull(); expect(view.queryByRole('dialog')).toBeNull(); expect(select).not.toHaveBeenCalled();
});
it('icon-only disclosure has an accessible name and a controlled content target', () => {
  const toggle = vi.fn();
  const view = render(<DisclosureRow icon={null} title="Details" open={false} expandable onToggle={toggle}>Body</DisclosureRow>);
  const button = view.getByRole('button', { name: 'Details' });
  const target = document.getElementById(button.getAttribute('aria-controls')!);
  expect(target?.hidden).toBe(true); fireEvent.click(button); expect(toggle).toHaveBeenCalledOnce();
});
