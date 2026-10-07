import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { useEffect, useState } from 'react';
import { applyTheme, readTheme } from '../src/app/appearance';
import { AppearanceRow } from '../src/presentation/settings/AppearanceRow';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); localStorage.clear(); document.body.removeAttribute('data-ds-dark-theme'); document.documentElement.style.colorScheme = ''; });
it('follows system changes only while selected, preserves preference, and releases the listener', () => {
  const listeners = new Set<() => void>(); let dark = true;
  vi.stubGlobal('matchMedia', vi.fn(() => ({ get matches() { return dark; },
    addEventListener: (_: string, listener: () => void) => listeners.add(listener),
    removeEventListener: (_: string, listener: () => void) => listeners.delete(listener) })));
  function Settings() {
    const [theme, setTheme] = useState(readTheme);
    useEffect(() => applyTheme(theme), [theme]);
    return <AppearanceRow title="Appearance" value={theme} onChange={setTheme} labels={{ light: 'Light', dark: 'Dark', system: 'System' }}/>;
  }
  const ui = render(<Settings/>);
  const changed = (value: boolean) => act(() => { dark = value; listeners.forEach(notify => notify()); });
  expect(ui.getByRole('button', { name: 'System' }).getAttribute('aria-pressed')).toBe('true');
  expect(document.body.hasAttribute('data-ds-dark-theme')).toBe(true);
  changed(false); expect(document.body.hasAttribute('data-ds-dark-theme')).toBe(false);
  expect(document.documentElement.style.colorScheme).toBe('light'); expect(readTheme()).toBe('system');
  fireEvent.click(ui.getByRole('button', { name: 'Dark' })); expect(listeners.size).toBe(0);
  changed(false); expect(document.body.hasAttribute('data-ds-dark-theme')).toBe(true); expect(readTheme()).toBe('dark');
  fireEvent.click(ui.getByRole('button', { name: 'Light' })); changed(true);
  expect(document.body.hasAttribute('data-ds-dark-theme')).toBe(false); expect(readTheme()).toBe('light');
  fireEvent.click(ui.getByRole('button', { name: 'System' }));
  expect(document.body.hasAttribute('data-ds-dark-theme')).toBe(true); expect(listeners.size).toBe(1);
  ui.unmount(); expect(listeners.size).toBe(0);
});
