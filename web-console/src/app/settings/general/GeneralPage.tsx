import { useConversationPreferences, TRANSCRIPT_MODES, type TranscriptMode } from '../../conversation-preferences';
import { useInteractiveHtml } from '../../components/workbench-documents/html/preferences';
import { localeController } from '../../../locale/controller';
import { useTranslation } from '../../../locale/react';
import type { Theme } from '../../appearance';
import { Switch } from '../../../presentation/primitives/Switch';
import { IconChevronUpOutline14, IconChevronDownOutline14 } from '../../../presentation/primitives/icons';
import { Choice } from '../primitives/aria';
import { useBusyEnter } from '../../composer/preferences';
import { SettingsPageShell, SettingsGroup, SettingsRow } from '../../../presentation/settings/SettingsPageShell';
import shellCss from '../../../presentation/settings/SettingsPageShell.module.css';
import preferenceCss from './ConversationPreferences.module.css';

/** The General product page.
 *
 * It holds the client-owned preferences, and only those that actually exist
 * today. Appearance, Language and busy-state sending are owned by this
 * browser client: it is deliberately not persisted as native configuration
 * merely to make ownership look uniform across the six pages, and no
 * preference is invented here to make the page look fuller than it is.
 *
 * Because everything here is client-owned rather than authored by a native
 * source, a Workspace has no General page at all — see `settingsPages`. Every
 * gesture writes its store directly, matching how DSH, pi-web and ZCode apply
 * scalar preferences: there is no staged draft and no Save affordance. */
export function GeneralPage({ theme, setTheme }: { theme: Theme; setTheme?: (theme: Theme) => void }) {
  const tx = useTranslation();
  const [busyEnter, busyEnterStore] = useBusyEnter();
  const [interactiveHtml, setInteractiveHtml] = useInteractiveHtml();
  const [preferences, preferenceStore] = useConversationPreferences();

  return <SettingsPageShell title={tx('settings:general-page.general')}>
    <SettingsGroup title={tx('settings:general-page.appearance')}>
      <SettingsRow
        label={tx('settings:general-page.appearance')}
        control={<div className={shellCss.scopeSegment} role="group" aria-label={tx('settings:general-page.appearance')}>
          {(['light', 'dark', 'system'] as const).map(id => <button key={id} type="button" aria-pressed={theme === id}
            onClick={() => setTheme?.(id)}>
            {tx(id === 'light' ? 'settings:copy.light' : id === 'dark' ? 'settings:copy.dark' : 'settings:appearance.system')}
          </button>)}
        </div>} />
      <SettingsRow label={tx('settings:copy.language')}
        control={<Choice hideLabel label={tx('settings:copy.language')} value={tx.locale}
          options={[['en', 'English'], ['zh', '中文']]} onChange={locale => localeController.setLocale(locale as 'en' | 'zh')} />} />
    </SettingsGroup>
    <SettingsGroup title={tx('settings:general-page.conversation')}>
      <SettingsRow label={tx('settings:general-page.font-size')} description={tx('settings:general-page.font-description')}
        control={<><div className={preferenceCss.stepper}>
          <output className={preferenceCss.value} aria-label={tx('settings:general-page.font-size')}>{preferences.fontSize}</output>
          <span className={preferenceCss.arrows}>
            <button type="button" className={preferenceCss.arrow} aria-label={tx('settings:general-page.font-increase')} disabled={preferences.fontSize >= 22} onClick={() => preferenceStore.update({ fontSize: preferences.fontSize + 1 })}><IconChevronUpOutline14 size={9} /></button>
            <button type="button" className={preferenceCss.arrow} aria-label={tx('settings:general-page.font-decrease')} disabled={preferences.fontSize <= 10} onClick={() => preferenceStore.update({ fontSize: preferences.fontSize - 1 })}><IconChevronDownOutline14 size={9} /></button>
          </span>
        </div><span className={preferenceCss.unit}>{tx('settings:general-page.font-unit')}</span></>} />
      <SettingsRow label={tx('settings:general-page.work-details')} description={tx('settings:general-page.work-description')}
        control={<Choice hideLabel label={tx('settings:general-page.work-details')} value={preferences.transcriptMode}
          options={TRANSCRIPT_MODES.map(mode => [mode, tx(`settings:general-page.${mode}`)] as const)}
          onChange={(transcriptMode: TranscriptMode) => preferenceStore.update({ transcriptMode })} />} />
      <SettingsRow label={tx('settings:general-page.coding-view')} description={tx('settings:general-page.coding-description')}
        control={<Switch label={tx('settings:general-page.coding-view')} checked={preferences.codingView} onChange={codingView => preferenceStore.update({ codingView })} />} />
      <SettingsRow label={tx('artifacts:workbench.html-interactive')} description={tx('artifacts:workbench.html-description')}
        control={<Choice hideLabel label={tx('artifacts:workbench.html-interactive')} value={interactiveHtml ? 'on' : 'off'}
          options={[['on', tx('artifacts:workbench.html-enabled')], ['off', tx('artifacts:workbench.html-static')]]}
          onChange={value => setInteractiveHtml(value === 'on')} />} />
      <SettingsRow label={tx('settings:general-page.busy-enter')} description={tx('settings:general-page.busy-enter-description')}
        control={<Choice hideLabel label={tx('settings:general-page.busy-enter')} value={busyEnter}
          options={[['queue', tx('settings:general-page.busy-enter-queue')], ['steer', tx('agent:submission.immediate')]]}
          onChange={next => busyEnterStore.setBusyEnter(next)} />} />
    </SettingsGroup>
  </SettingsPageShell>;
}
