import { useEffect } from 'react';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { DialogSurface, OverlayScope } from '../src/presentation/primitives/DialogSurface';

afterEach(cleanup);

it('a retained page hides its requested dialog without discarding its input or trapping outside focus', async () => {
  const close = vi.fn();
  const page = (active: boolean) => <><button>Home</button><OverlayScope active={active}>
    <DialogSurface open onClose={close} title="Retained editor" overlayClassName="overlay">
      <label>Local text<input defaultValue="Original"/></label>
    </DialogSurface>
  </OverlayScope></>;
  const ui = render(page(true));
  const input = await screen.findByLabelText('Local text') as HTMLInputElement;
  fireEvent.change(input, { target: { value: 'Authored before hiding' } });
  ui.rerender(page(false));
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  expect(input.isConnected).toBe(true);
  screen.getByRole('button', { name: 'Home' }).focus();
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Home' }));
  expect(close).not.toHaveBeenCalled();
  ui.rerender(page(true));
  expect(await screen.findByLabelText('Local text')).toBe(input);
  expect(input.value).toBe('Authored before hiding');
});

it('hiding a page does not mount the bodies of confirmations it never opened', () => {
  const mounted = vi.fn();
  function ConfirmationBody() { useEffect(mounted, []); return <p>Confirm removal</p>; }
  const page = (active: boolean) => <OverlayScope active={active}>
    <DialogSurface open={false} onClose={() => {}} title="Removal" overlayClassName="overlay"><ConfirmationBody/></DialogSurface>
  </OverlayScope>;
  const ui = render(page(true));
  ui.rerender(page(false));
  expect(mounted).not.toHaveBeenCalled();
  expect(screen.queryByText('Confirm removal')).toBeNull();
});
