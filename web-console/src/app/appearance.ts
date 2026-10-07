export type Theme = 'light' | 'dark' | 'system';
const KEY = 'rustx-appearance-v1';
/** Only a presentation preference; no native configuration or credentials. */
export function readTheme(): Theme {
  try { const value = localStorage.getItem(KEY); return value === 'light' || value === 'dark' ? value : 'system'; } catch { return 'system'; }
}
/** Own the system subscription for exactly the lifetime of this preference. */
export function applyTheme(theme: Theme) {
  const system = theme === 'system' ? window.matchMedia?.('(prefers-color-scheme: dark)') : undefined;
  const update = () => {
    const dark = theme === 'dark' || theme === 'system' && !!system?.matches;
    document.body.toggleAttribute('data-ds-dark-theme', dark);
    document.documentElement.style.colorScheme = dark ? 'dark' : 'light';
  };
  update();
  system?.addEventListener('change', update);
  try { localStorage.setItem(KEY, theme); } catch { /* Optional preference. */ }
  return () => system?.removeEventListener('change', update);
}
