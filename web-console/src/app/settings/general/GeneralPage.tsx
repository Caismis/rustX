import { localeController } from '../../../locale/controller';
import { useBusyEnter } from '../../composer/preferences';
import { useTranslation } from '../../../locale/react';
import { Choice } from '../primitives/aria';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The General product page.
 *
 * It holds the client-owned preferences, and only those that actually exist
 * today. Appearance, Language and the Composer's busy-state Enter behavior
 * (DeepSeek Harness General `composer-enter` row) are owned by this browser client: it is deliberately not
 * persisted as native configuration merely to make ownership look uniform
 * across the six pages, and no preference is invented here to make the page
 * look fuller than it is.
 *
 * Because everything here is client-owned rather than authored by a native
 * source, a Workspace has no General page at all — see `settingsPages`. */
export function GeneralPage({ theme, setTheme }: { theme: 'light' | 'dark'; setTheme?: (theme: 'light' | 'dark') => void }) {
  const tx = useTranslation();
  const [busyEnter, composer] = useBusyEnter();
  return <section aria-label={tx('settings:general-page.general')}>
    <h3>{tx('settings:general-page.general')}</h3>
    <p>{tx('settings:general-page.preferences-this-browser-client-owns-they-are-stored-by-the-clie')}</p>
    <Choice label={tx('settings:copy.language')} value={tx.locale} options={[["en", "English"], ["zh", "中文"]]} onChange={localeController.setLocale} />
    <h4>{tx('settings:general-page.appearance')}</h4>
    <Choice label={tx('settings:general-page.theme')} value={theme} options={[['light', tx('settings:copy.light')], ['dark', tx('settings:copy.dark')]]}
      onChange={value => setTheme?.(value)} />
    <h4>{tx('settings:general-page.conversation')}</h4>
    <Choice label={tx('settings:general-page.busy-enter')} value={busyEnter}
      options={[['queue', tx('settings:general-page.busy-enter-queue')], ['steer', tx('settings:general-page.busy-enter-steer')]]}
      onChange={value => composer.setBusyEnter(value)}
      description={tx('settings:general-page.busy-enter-description')} />
    <p className={css.hint}>{tx('settings:general-page.connection-is-also-client-owned-it-is-kept-with-the-other-diagno')}</p>
  </section>;
}
