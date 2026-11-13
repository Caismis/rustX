import { act, cleanup, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { SettingsFeedback, SettingsNotice } from '../src/app/settings/SettingsFeedback';

afterEach(() => { cleanup(); vi.useRealTimers(); });

it('shows a settings error as a temporary toast without adding text to the form', async () => {
  vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout'] });
  const editor = (attempt: number) => <SettingsFeedback>
    <form aria-label="Provider settings">
      <input aria-label="Base URL" defaultValue="invalid" aria-invalid="true"/>
      <SettingsNotice text="Enter a valid Base URL." occurrence={attempt}/>
    </form>
  </SettingsFeedback>;
  const ui = render(editor(1));
  const form = screen.getByRole('form', { name: 'Provider settings' });
  expect(within(form).queryByText('Enter a valid Base URL.')).toBeNull();
  expect(screen.getByText('Enter a valid Base URL.').closest('[data-toast]')).not.toBeNull();
  await act(async () => { await vi.advanceTimersByTimeAsync(6100); });
  expect(screen.queryByText('Enter a valid Base URL.')).toBeNull();
  expect(within(form).getByLabelText('Base URL')).toHaveProperty('value', 'invalid');
  ui.rerender(editor(1));
  expect(screen.queryByText('Enter a valid Base URL.')).toBeNull();
  ui.rerender(editor(2));
  expect(screen.getByText('Enter a valid Base URL.')).toBeTruthy();
});
