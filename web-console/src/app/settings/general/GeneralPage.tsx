import { localeController } from '../../../locale/controller';
import { useTranslation } from '../../../locale/react';
import { Choice } from '../primitives/aria';
import { Label, Radio, RadioGroup } from 'react-aria-components';
import appearance from './GeneralPage.module.css';

/** The General product page.
 *
 * It holds the client-owned preferences, and only those that actually exist
 * today. Appearance and Language are owned by this browser client: it is deliberately not
 * persisted as native configuration merely to make ownership look uniform
 * across the six pages, and no preference is invented here to make the page
 * look fuller than it is.
 *
 * Because everything here is client-owned rather than authored by a native
 * source, a Workspace has no General page at all — see `settingsPages`. */
export function GeneralPage({ theme, setTheme }: { theme: 'light' | 'dark'; setTheme?: (theme: 'light' | 'dark') => void }) {
  const tx = useTranslation();
  return <section aria-label={tx('settings:general-page.general')}>
    <h3>{tx('settings:general-page.general')}</h3>
    <p>{tx('settings:general-page.preferences-this-browser-client-owns-they-are-stored-by-the-clie')}</p>
    <Choice label={tx('settings:copy.language')} value={tx.locale} options={[["en", "English"], ["zh", "中文"]]} onChange={localeController.setLocale} />
    <RadioGroup className={appearance.themes} value={theme} onChange={value => { if (value === 'light' || value === 'dark') setTheme?.(value); }}>
      <Label className={appearance.label}>{tx('settings:general-page.theme')}</Label>
      {(['light', 'dark'] as const).map(value => <Radio key={value} value={value} className={appearance.theme}>
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
          {value === 'light' ? <><circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5"/></> : <path d="M20.5 13.5A8.5 8.5 0 0 1 10.5 3a8.5 8.5 0 1 0 10 10.5Z"/>}
        </svg>
        <span>{tx(`settings:copy.${value}`)}</span>
      </Radio>)}
    </RadioGroup>
  </section>;
}
