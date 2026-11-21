import { useEffect, useRef, useState } from 'react';
import { useConversationPreferences, TRANSCRIPT_MODES, type TranscriptMode } from '../../conversation-preferences';
import { useInteractiveHtml } from '../../components/workbench-documents/html/preferences';
import { localeController } from '../../../locale/controller';
import { useTranslation } from '../../../locale/react';
import type { Theme } from '../../appearance';
import { Switch } from '../../../presentation/primitives/Switch';
import { IconChevronUpOutline14, IconChevronDownOutline14 } from '../../../presentation/primitives/icons';
import { Choice } from '../primitives/aria';
import { useBusyEnter, type BusyEnterBehavior } from '../../composer/preferences';
import { SettingsPageShell, SettingsGroup, SettingsRow } from '../../../presentation/settings/SettingsPageShell';
import { PageScopeToolbar, PageRefreshButton } from '../ScopeToolbar';
import { PageTransactionFooter, usePageTransaction } from '../forms/bridge';
import shellCss from '../../../presentation/settings/SettingsPageShell.module.css';
import preferenceCss from './ConversationPreferences.module.css';

/** A staged copy of every client preference this page presents. `undefined`
 * fields fall back to the live store value; saving applies the staged ones
 * back to their stores. Nothing is persisted from a render or an edit. */
interface PreferenceDraft {
  theme: Theme; locale: string; fontSize: number; transcriptMode: TranscriptMode;
  codingView: boolean; busyEnter: BusyEnterBehavior; interactiveHtml: boolean;
}

/** The General product page.
 *
 * It holds the client-owned preferences, and only those that actually exist
 * today. Appearance, Language and busy-state sending are owned by this
 * browser client: it is deliberately not persisted as native configuration
 * merely to make ownership look uniform across the six pages, and no
 * preference is invented here to make the page look fuller than it is.
 *
 * Because everything here is client-owned rather than authored by a native
 * source, a Workspace has no General page at all — see `settingsPages`.
 * Edits stage into the page transaction like every other page: the stores
 * are written only by the footer's Save. */
export function GeneralPage({ theme, setTheme }: { theme: Theme; setTheme?: (theme: Theme) => void }) {
  const tx = useTranslation();
  const page = usePageTransaction();
  const [busyEnter, busyEnterStore] = useBusyEnter();
  const [interactiveHtml, setInteractiveHtml] = useInteractiveHtml();
  const [preferences, preferenceStore] = useConversationPreferences();
  const base: PreferenceDraft = {
    theme, locale: tx.locale, fontSize: preferences.fontSize, transcriptMode: preferences.transcriptMode,
    codingView: preferences.codingView, busyEnter, interactiveHtml,
  };
  const [edits, setEdits] = useState<Partial<PreferenceDraft>>({});
  const draft: PreferenceDraft = { ...base, ...edits };
  const dirty = (Object.keys(edits) as (keyof PreferenceDraft)[]).some(key => edits[key] !== base[key]);

  const stage = (patch: Partial<PreferenceDraft>) => setEdits(current => ({ ...current, ...patch }));
  const saveRef = useRef<(() => void) | undefined>(undefined);
  const cancelRef = useRef<(() => void) | undefined>(undefined);
  saveRef.current = () => {
    if (edits.theme !== undefined) setTheme?.(edits.theme);
    if (edits.locale !== undefined) localeController.setLocale(edits.locale as 'en' | 'zh');
    if (edits.fontSize !== undefined || edits.transcriptMode !== undefined || edits.codingView !== undefined)
      preferenceStore.update({ fontSize: edits.fontSize, transcriptMode: edits.transcriptMode, codingView: edits.codingView });
    if (edits.busyEnter !== undefined) busyEnterStore.setBusyEnter(edits.busyEnter);
    if (edits.interactiveHtml !== undefined) setInteractiveHtml(edits.interactiveHtml);
    setEdits({});
  };
  cancelRef.current = () => setEdits({});
  useEffect(() => {
    page?.setLocal(dirty, () => saveRef.current?.(), () => cancelRef.current?.());
  }, [page, dirty]);

  return <SettingsPageShell title={tx('settings:general-page.general')} toolbar={<><PageScopeToolbar /><PageRefreshButton /></>} footer={<PageTransactionFooter />}>
    <SettingsGroup title={tx('settings:general-page.appearance')}>
      <SettingsRow
        label={tx('settings:general-page.appearance')}
        control={<div className={shellCss.scopeSegment} role="group" aria-label={tx('settings:general-page.appearance')}>
          {(['light', 'dark', 'system'] as const).map(id => <button key={id} type="button" aria-pressed={draft.theme === id}
            onClick={() => stage({ theme: id })}>
            {tx(id === 'light' ? 'settings:copy.light' : id === 'dark' ? 'settings:copy.dark' : 'settings:appearance.system')}
          </button>)}
        </div>} />
      <SettingsRow label={tx('settings:copy.language')}
        control={<Choice hideLabel label={tx('settings:copy.language')} value={draft.locale}
          options={[['en', 'English'], ['zh', '中文']]} onChange={locale => stage({ locale })} />} />
    </SettingsGroup>
    <SettingsGroup title={tx('settings:general-page.conversation')}>
      <SettingsRow label={tx('settings:general-page.font-size')} description={tx('settings:general-page.font-description')}
        control={<><div className={preferenceCss.stepper}>
          <output className={preferenceCss.value} aria-label={tx('settings:general-page.font-size')}>{draft.fontSize}</output>
          <span className={preferenceCss.arrows}>
            <button type="button" className={preferenceCss.arrow} aria-label={tx('settings:general-page.font-increase')} disabled={draft.fontSize >= 22} onClick={() => stage({ fontSize: draft.fontSize + 1 })}><IconChevronUpOutline14 size={9} /></button>
            <button type="button" className={preferenceCss.arrow} aria-label={tx('settings:general-page.font-decrease')} disabled={draft.fontSize <= 10} onClick={() => stage({ fontSize: draft.fontSize - 1 })}><IconChevronDownOutline14 size={9} /></button>
          </span>
        </div><span className={preferenceCss.unit}>{tx('settings:general-page.font-unit')}</span></>} />
      <SettingsRow label={tx('settings:general-page.work-details')} description={tx('settings:general-page.work-description')}
        control={<Choice hideLabel label={tx('settings:general-page.work-details')} value={draft.transcriptMode}
          options={TRANSCRIPT_MODES.map(mode => [mode, tx(`settings:general-page.${mode}`)] as const)}
          onChange={transcriptMode => stage({ transcriptMode })} />} />
      <SettingsRow label={tx('settings:general-page.coding-view')} description={tx('settings:general-page.coding-description')}
        control={<Switch label={tx('settings:general-page.coding-view')} checked={draft.codingView} onChange={codingView => stage({ codingView })} />} />
      <SettingsRow label={tx('artifacts:workbench.html-interactive')} description={tx('artifacts:workbench.html-description')}
        control={<Choice hideLabel label={tx('artifacts:workbench.html-interactive')} value={draft.interactiveHtml ? 'on' : 'off'}
          options={[['on', tx('artifacts:workbench.html-enabled')], ['off', tx('artifacts:workbench.html-static')]]}
          onChange={value => stage({ interactiveHtml: value === 'on' })} />} />
      <SettingsRow label={tx('settings:general-page.busy-enter')} description={tx('settings:general-page.busy-enter-description')}
        control={<Choice hideLabel label={tx('settings:general-page.busy-enter')} value={draft.busyEnter}
          options={[['queue', tx('settings:general-page.busy-enter-queue')], ['steer', tx('agent:submission.immediate')]]}
          onChange={busyEnter => stage({ busyEnter })} />} />
    </SettingsGroup>
  </SettingsPageShell>;
}
