import { ConversationPreferenceRows } from './ConversationPreferenceRows';
import { useInteractiveHtml } from '../../components/workbench-documents/html/preferences';
import { localeController } from '../../../locale/controller';
import { useBusyEnter } from '../../composer/preferences';
import { useTranslation } from '../../../locale/react';
import type { Theme } from '../../appearance';
import { AppearanceRow } from '../../../presentation/settings/AppearanceRow';
import { Choice } from '../primitives/aria';

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
export function GeneralPage({ theme, setTheme }: { theme: Theme; setTheme?: (theme: Theme) => void }) {
  const tx = useTranslation();
  const [interactiveHtml, setInteractiveHtml] = useInteractiveHtml();
  const [busyEnter, composer] = useBusyEnter();
  return <section aria-label={tx('settings:general-page.general')}>
    <h3>{tx('settings:general-page.general')}</h3>
    <Choice label={tx('settings:copy.language')} value={tx.locale} options={[["en", "English"], ["zh", "中文"]]} onChange={localeController.setLocale} />
    <AppearanceRow title={tx('settings:general-page.appearance')} value={theme} onChange={setTheme}
      labels={{ light: tx('settings:copy.light'), dark: tx('settings:copy.dark'), system: tx('settings:appearance.system') }}/>
    <ConversationPreferenceRows/>
    <Choice label={tx('artifacts:workbench.html-interactive')} value={interactiveHtml ? 'on' : 'off'}
      options={[["on", tx('artifacts:workbench.html-enabled')], ["off", tx('artifacts:workbench.html-static')]]}
      onChange={value => setInteractiveHtml(value === 'on')} description={tx('artifacts:workbench.html-description')} />
    <h4>{tx('settings:general-page.conversation')}</h4>
    <Choice label={tx('settings:general-page.busy-enter')} value={busyEnter}
      options={[['queue', tx('settings:general-page.busy-enter-queue')], ['steer', tx('settings:general-page.busy-enter-steer')]]}
      onChange={value => composer.setBusyEnter(value)}
      description={tx('settings:general-page.busy-enter-description')} />
  </section>;
}
