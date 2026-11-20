/* Copyright (c) 2026 DeepSeek. MIT. Adapted General preference rows; see PROVENANCE.md. */
import {
  useConversationPreferences,
  TRANSCRIPT_MODES,
  COLLAPSE_TIMINGS,
  FOLDED_REPLIES,
  PERFORMANCE_USAGE_MODES,
  type TranscriptMode,
  type CollapseTiming,
  type FoldedReplies,
  type PerformanceUsageMode,
} from '../../conversation-preferences';
import { Switch } from '../../../presentation/primitives/Switch';
import { IconChevronUpOutline14, IconChevronDownOutline14 } from '../../../presentation/primitives/icons';
import preferenceCss from './ConversationPreferences.module.css';
import { useId, useRef, useState } from 'react';
import { Menu } from '../../../presentation/primitives/Menu';
import { useTranslation } from '../../../locale/react';

function PreferenceSelectRow<T extends string>({
  idPrefix,
  title,
  description,
  value,
  options,
  onSelect,
}: {
  idPrefix: string;
  title: string;
  description: string;
  value: T;
  options: readonly { id: T; label: string }[];
  onSelect: (value: T) => void;
}) {
  const [open, setOpen] = useState(false);
  const selector = useRef<HTMLButtonElement>(null);
  const uid = useId();
  const labelId = `${idPrefix}-label-${uid}`;
  const valueId = `${idPrefix}-value-${uid}`;
  const descId = `${idPrefix}-desc-${uid}`;
  const selectedLabel = options.find(option => option.id === value)?.label ?? value;
  return (
    <div className={preferenceCss.row}>
      <div className={preferenceCss.rowText}>
        <div id={labelId} className={preferenceCss.title}>{title}</div>
        <div id={descId} className={preferenceCss.desc}>{description}</div>
      </div>
      <Menu
        open={open}
        onClose={() => setOpen(false)}
        align="end"
        autoFocus
        selectedId={value}
        items={options}
        onSelect={id => {
          selector.current?.focus({ preventScroll: true });
          setOpen(false);
          onSelect(id as T);
        }}
        anchor={
          <button
            ref={selector}
            type="button"
            className={preferenceCss.selector}
            aria-labelledby={`${labelId} ${valueId}`}
            aria-describedby={descId}
            aria-haspopup="menu"
            aria-expanded={open}
            onClick={() => setOpen(previous => !previous)}
          >
            <span id={valueId}>{selectedLabel}</span>
            <IconChevronDownOutline14 />
          </button>
        }
      />
    </div>
  );
}

export function ConversationPreferenceRows() {
  const tx = useTranslation();
  const [preferences, preferenceStore] = useConversationPreferences();
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
    <PreferenceSelectRow
      idPrefix="work-details"
      title={tx('settings:general-page.work-details')}
      description={tx('settings:general-page.work-description')}
      value={preferences.transcriptMode}
      options={TRANSCRIPT_MODES.map(mode => ({ id: mode, label: tx(`settings:general-page.${mode}`) }))}
      onSelect={(transcriptMode: TranscriptMode) => preferenceStore.update({ transcriptMode })}
    />
    <PreferenceSelectRow
      idPrefix="collapse-timing"
      title={tx('settings:general-page.collapse-timing')}
      description={tx('settings:general-page.collapse-timing-description')}
      value={preferences.collapseTiming}
      options={COLLAPSE_TIMINGS.map(timing => ({
        id: timing,
        label: tx(timing === 'next-input' ? 'settings:general-page.collapse-next-input' : 'settings:general-page.collapse-completion'),
      }))}
      onSelect={(collapseTiming: CollapseTiming) => preferenceStore.update({ collapseTiming })}
    />
    {preferences.transcriptMode !== 'verbose' && (
      <PreferenceSelectRow
        idPrefix="folded-replies"
        title={tx('settings:general-page.folded-replies')}
        description={tx('settings:general-page.folded-replies-description')}
        value={preferences.foldedReplies}
        options={FOLDED_REPLIES.map(mode => ({
          id: mode,
          label: tx(mode === 'final' ? 'settings:general-page.folded-replies-final' : 'settings:general-page.folded-replies-all'),
        }))}
        onSelect={(foldedReplies: FoldedReplies) => preferenceStore.update({ foldedReplies })}
      />
    )}
    <PreferenceSelectRow
      idPrefix="performance-usage"
      title={tx('settings:general-page.performance-usage')}
      description={tx('settings:general-page.performance-usage-description')}
      value={preferences.performanceUsage}
      options={PERFORMANCE_USAGE_MODES.map(mode => ({
        id: mode,
        label: tx(mode === 'compact' ? 'settings:general-page.performance-compact' : 'settings:general-page.performance-detailed'),
      }))}
      onSelect={(performanceUsage: PerformanceUsageMode) => preferenceStore.update({ performanceUsage })}
    />
    <div className={preferenceCss.row}>
      <div className={preferenceCss.rowText}><div className={preferenceCss.title}>{tx('settings:general-page.coding-view')}</div><div className={preferenceCss.desc}>{tx('settings:general-page.coding-description')}</div></div>
      <Switch label={tx('settings:general-page.coding-view')} checked={preferences.codingView} onChange={codingView => preferenceStore.update({codingView})}/>
    </div>
  </div>;
}
