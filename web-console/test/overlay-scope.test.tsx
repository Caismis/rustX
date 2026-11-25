import { useEffect, useRef, useState } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
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

it('does not steal focus that moved elsewhere after a dialog closes', async () => {
  function Harness() {
    const [open, setOpen] = useState(true);
    const target = useRef<HTMLButtonElement>(null), other = useRef<HTMLButtonElement>(null);
    return <><button ref={target}>Settlement target</button><button ref={other}>Newer focus</button>
      <DialogSurface open={open} onClose={() => setOpen(false)} title="Confirm" overlayClassName="overlay" finalFocus={() => target.current ?? false}>
        <button onClick={() => setOpen(false)}>Close</button>
      </DialogSurface></>;
  }
  render(<Harness/>);
  fireEvent.click(await screen.findByRole('button', { name: 'Close' }));
  const other = screen.getByRole('button', { name: 'Newer focus' });
  other.focus();
  await act(async () => new Promise(resolve => setTimeout(resolve, 20)));
  expect(document.activeElement).toBe(other);
});
