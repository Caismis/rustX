/* Copyright (c) 2026 DeepSeek. MIT. Harness Run time controls adapted to native timing; see PROVENANCE.md. */
import { useId, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { ComboBox, Input, Button as ComboButton, ListBox, ListBoxItem, Popover } from 'react-aria-components';
import { Menu } from '../../presentation/primitives/Menu';
import { Button } from '../../presentation/primitives/Button';
import { IconChevronDownOutline14, IconClockOutline16 } from '../../presentation/primitives/icons';
import { DatePicker } from './DatePicker';
import { ClockPicker } from './ClockPicker';
import { IconCalendarOutlineRegular } from './CalendarIcon';
import type { TaskTranslate } from './copy';
import { browserZone, secondPrecision, zoneLabel, type TimingDraft, type TimingKind } from './timing';
import { cronPreview, parseCronExpression } from './cron';
import css from './TaskManager.module.css';
import custom from './scheduler.module.css';

const choices = ['weekly', 'weekdays', 'daily', 'hour', 'minute', 'second', 'once', 'cron'] as const;
type Choice = typeof choices[number];
export function TimingEditor({ value, change, disabled, t, locale }: {
  value: TimingDraft; change: (value: TimingDraft) => void; disabled: boolean; t: TaskTranslate; locale: string;
}) {
  const id = useId();
  const [menu, setMenu] = useState(false), [picker, setPicker] = useState<'date' | 'time'>();
  const date = useRef<HTMLButtonElement>(null), clock = useRef<HTMLButtonElement>(null);
  const update = (patch: Partial<TimingDraft>) => change({ ...value, ...patch });
  const selected: Choice = value.kind === 'interval' ? value.unit : value.kind;
  const label = (choice: Choice) => t(choice === 'hour' ? 'rule.everyHours' : choice === 'minute' ? 'rule.everyMinutes' : choice === 'second' ? 'rule.everySeconds'
    : choice === 'once' ? 'timing.once' : choice === 'cron' ? 'timing.cron' : choice === 'weekly' ? 'timing.weekly' : choice === 'weekdays' ? 'timing.weekdays' : 'timing.daily');
  const select = (choice: Choice) => {
    setMenu(false);
    if (choice === 'hour' || choice === 'minute' || choice === 'second') update({ kind: 'interval', unit: choice, quantity: selected === choice ? value.quantity : choice === 'second' ? '60' : '1' });
    else update({ kind: choice as TimingKind, time: choice === 'once' ? value.time : secondPrecision(value.time).slice(0, 5) + ':00' });
  };
  const parsed = value.kind === 'cron' ? parseCronExpression(value.expression) : undefined;
  const zones = useMemo(() => {
    const system = browserZone(), names = new Set(['UTC', system, value.zone, ...Intl.supportedValuesOf('timeZone')]);
    return [...names].map(id => { const label = zoneLabel(id, locale) + ' · ' + id; return { id, label, search: label }; });
  }, [value.zone, locale]);
  const [zoneQuery, setZoneQuery] = useState<string | undefined>();
  return <section className={css.ruleCard} aria-label={t('timing.title')}>
    <div className={css.ruleRows}>
      <div className={css.ruleRow}><span className={css.ruleLabel}>{t('timing.repeat')}</span>
        <Menu open={menu} onClose={() => setMenu(false)} items={choices.map(id => ({ id, label: label(id) }))} selectedId={selected} onSelect={id => select(id as Choice)}
          anchor={<Button size="sm" className={css.ruleCurrent} disabled={disabled} aria-label={t('timing.repeat')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}>{label(selected)}<IconChevronDownOutline14/></Button>}/>
      </div>
      {value.kind === 'interval' ? <div className={css.ruleRow}><label className={css.ruleLabel} htmlFor={id + '-quantity'}>{t('timing.number')}</label>
        <div className={css.ruleInterval}><input id={id + '-quantity'} className={css.ruleIntervalInput} type="number" min={value.unit === 'second' ? 60 : 1} step="any" disabled={disabled} value={value.quantity} onChange={event => update({ quantity: event.target.value })}/>
          <span className={css.ruleIntervalUnit}>{t(value.unit === 'hour' ? 'timing.hours' : value.unit === 'minute' ? 'timing.minutes' : 'timing.seconds')}</span></div>
      </div> : <>
        {value.kind === 'weekly' && <div className={css.ruleRow}><span className={css.ruleLabel}>{t('timing.weekday')}</span><div className={css.ruleWeekdays} role="group" aria-label={t('timing.weekday')}>
          {([1, 2, 3, 4, 5, 6, 7] as const).map(day => <button key={day} type="button" disabled={disabled} aria-pressed={value.weekdays.includes(day)} onClick={() => update({ weekdays: value.weekdays.includes(day) ? value.weekdays.filter(value => value !== day) : [...value.weekdays, day] })}>{t(('frequency.weekday.' + day) as 'frequency.weekday.1')}</button>)}
        </div></div>}
        {value.kind === 'once' && <div className={css.ruleRow}><span className={css.ruleLabel}>{t('timing.date')}</span>
          <button ref={date} type="button" className={clsx(css.pickerTrigger, custom.pickerButton)} disabled={disabled} aria-label={t('timing.date')} aria-haspopup="dialog" aria-expanded={picker === 'date'} onClick={() => setPicker(picker === 'date' ? undefined : 'date')}><span>{value.date.replaceAll('-', '/')}</span><IconCalendarOutlineRegular/></button>
          <DatePicker open={picker === 'date'} anchorRef={date} value={value.date} onPick={date => update({ date })} onClose={() => setPicker(undefined)} t={t}/>
        </div>}
        {value.kind !== 'cron' && <div className={css.ruleRow}><span className={css.ruleLabel}>{t('timing.time')}</span>
          <button ref={clock} type="button" className={clsx(css.pickerTrigger, custom.pickerButton)} disabled={disabled} aria-label={t('timing.time')} aria-haspopup="dialog" aria-expanded={picker === 'time'} onClick={() => setPicker(picker === 'time' ? undefined : 'time')}><span>{value.kind === 'once' ? secondPrecision(value.time) : value.time.slice(0, 5)}</span><IconClockOutline16/></button>
          <ClockPicker open={picker === 'time'} anchorRef={clock} value={value.time} seconds={value.kind === 'once'} onPick={time => update({ time })} onClose={() => setPicker(undefined)} t={t}/>
        </div>}
        {value.kind === 'cron' && <div className={css.ruleRow}><label className={css.ruleLabel} htmlFor={id + '-cron'}>{t('timing.expression')}</label><div className={css.ruleControl}>
          <input id={id + '-cron'} className={css.ruleInput} disabled={disabled} value={value.expression} spellCheck={false} onChange={event => update({ expression: event.target.value })}/>
          <p className={parsed ? css.ruleHint : css.ruleHintError}>{parsed ? cronPreview(parsed, t, locale) : t('timing.invalid-cron')}</p>
        </div></div>}
        <div className={css.ruleRow}><span className={css.ruleLabel}>{t('timing.zone')}</span>
          <ComboBox className={custom.zoneCombo} aria-label={t('timing.zone')} isDisabled={disabled} selectedKey={value.zone} inputValue={zoneQuery ?? zoneLabel(value.zone, locale)}
            onInputChange={setZoneQuery} onSelectionChange={key => { if (typeof key === 'string') update({ zone: key }); setZoneQuery(undefined); }}
            defaultFilter={(text, input) => text.toLocaleLowerCase().includes(input.toLocaleLowerCase())} menuTrigger="focus" allowsEmptyCollection>
            <div className={custom.zoneInput}><Input placeholder={t('timing.zone-search')} onBlur={() => setZoneQuery(undefined)}/><ComboButton aria-label={t('timing.zone-search')}><IconChevronDownOutline14/></ComboButton></div>
            <Popover className={custom.zonePopup} placement="bottom end"><ListBox items={zones} className={custom.zoneList} aria-label={t('timing.zone')}>
              {zone => <ListBoxItem id={zone.id} textValue={zone.search} className={custom.zoneOption}>{zone.label}</ListBoxItem>}
            </ListBox></Popover>
          </ComboBox>
        </div>
      </>}
    </div>
    {value.kind === 'once' && <p className={css.ruleHint}>{t('timing.timezone-note')}</p>}
  </section>;
}
