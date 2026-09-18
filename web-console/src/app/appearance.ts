export type Theme = 'light' | 'dark';
const KEY = 'rustx-appearance-v1';
/** Only a presentation preference; no native configuration or credentials. */
export function readTheme(): Theme {
  try { return localStorage.getItem(KEY) === 'dark' ? 'dark' : 'light'; } catch { return 'light'; }
}
export function applyTheme(theme: Theme) {
  document.body.toggleAttribute('data-ds-dark-theme', theme === 'dark');
  document.documentElement.style.colorScheme = theme;
  try { localStorage.setItem(KEY, theme); } catch { /* Optional preference. */ }
}
