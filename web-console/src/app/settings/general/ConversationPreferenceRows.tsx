/* Copyright (c) 2026 DeepSeek. MIT. Adapted General preference rows; see PROVENANCE.md. */
import { useConversationPreferences, TRANSCRIPT_MODES, type TranscriptMode } from '../../conversation-preferences';
import { Switch } from '../../../presentation/primitives/Switch';
import { IconChevronUpOutline14, IconChevronDownOutline14 } from '../../../presentation/primitives/icons';
import preferenceCss from './ConversationPreferences.module.css';
import { useRef, useState } from 'react';
import { Menu } from '../../../presentation/primitives/Menu';
import { useTranslation } from '../../../locale/react';

export function ConversationPreferenceRows() {
  const tx = useTranslation();
  const [preferences, preferenceStore] = useConversationPreferences();
  const [menuOpen, setMenuOpen] = useState(false);
  const selector = useRef<HTMLButtonElement>(null);
  return <div>
    <div className={preferenceCss.row}>
      <div className={preferenceCss.rowText}><div className={preferenceCss.title}>{tx('settings:general-page.font-size')}</div><div className={preferenceCss.desc}>{tx('settings:general-page.font-description')}</div></div>
      <div className={preferenceCss.control}><div className={preferenceCss.stepper}>
        <output className={preferenceCss.value} aria-label={tx('settings:general-page.font-size')}>{preferences.fontSize}</output>
        <span className={preferenceCss.arrows}>
          <button type="button" className={preferenceCss.arrow} aria-label={tx('settings:general-page.font-increase')} disabled={preferences.fontSize >= 22} onClick={() => preferenceStore.update({fontSize: preferences.fontSize + 1})}><IconChevronUpOutline14 size={9}/></button>
          <button type="button" className={preferenceCss.arrow} aria-label={tx('settings:general-page.font-decrease')} disabled={preferences.fontSize <= 10} onClick={() => preferenceStore.update({fontSize: preferences.fontSize - 1})}><IconChevronDownOutline14 size={9}/></button>
        </span>
      </div><span className={preferenceCss.unit}>{tx('settings:general-page.font-unit')}</span></div>
    </div>
    <div className={preferenceCss.row}>
      <div className={preferenceCss.rowText}><div id="work-details-label" className={preferenceCss.title}>{tx('settings:general-page.work-details')}</div><div id="work-details-description" className={preferenceCss.desc}>{tx('settings:general-page.work-description')}</div></div>
      <Menu open={menuOpen} onClose={() => setMenuOpen(false)} align="end" autoFocus selectedId={preferences.transcriptMode}
        items={TRANSCRIPT_MODES.map(mode => ({id: mode, label: tx(`settings:general-page.${mode}`)}))}
        onSelect={id => { selector.current?.focus({preventScroll:true}); setMenuOpen(false); preferenceStore.update({transcriptMode: id as TranscriptMode}); }}
        anchor={<button ref={selector} type="button" className={preferenceCss.selector} aria-labelledby="work-details-label work-details-value" aria-describedby="work-details-description" aria-haspopup="menu" aria-expanded={menuOpen} onClick={() => setMenuOpen(value => !value)}><span id="work-details-value">{tx(`settings:general-page.${preferences.transcriptMode}`)}</span><IconChevronDownOutline14/></button>}/>

    </div>
    <div className={preferenceCss.row}>
      <div className={preferenceCss.rowText}><div className={preferenceCss.title}>{tx('settings:general-page.coding-view')}</div><div className={preferenceCss.desc}>{tx('settings:general-page.coding-description')}</div></div>
      <Switch label={tx('settings:general-page.coding-view')} checked={preferences.codingView} onChange={codingView => preferenceStore.update({codingView})}/>
    </div>
  </div>;
}
