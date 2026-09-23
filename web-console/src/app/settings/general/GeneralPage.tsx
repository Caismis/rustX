import { Choice } from '../primitives/aria';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The General product page.
 *
 * It holds the client-owned preferences, and only those that actually exist
 * today. Appearance is owned by this browser client: it is deliberately not
 * persisted as native configuration merely to make ownership look uniform
 * across the six pages, and no preference is invented here to make the page
 * look fuller than it is.
 *
 * Because everything here is client-owned rather than authored by a native
 * source, a Workspace has no General page at all — see `settingsPages`. */
export function GeneralPage({ theme, setTheme }: { theme: 'light' | 'dark'; setTheme?: (theme: 'light' | 'dark') => void }) {
  return <section aria-label="General">
    <h3>General</h3>
    <p>Preferences this browser client owns. They are stored by the client and are never written to a native configuration source.</p>
    <h4>Appearance</h4>
    <Choice label="Theme" value={theme} options={[['light', 'Light'], ['dark', 'Dark']]}
      onChange={value => setTheme?.(value)} />
    <p className={css.hint}>Connection is also client-owned; it is kept with the other diagnostics on Advanced.</p>
  </section>;
}
