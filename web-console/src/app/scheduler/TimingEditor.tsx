/* Copyright (c) 2026 DeepSeek. MIT. Harness Run time controls adapted to native timing; see PROVENANCE.md. */
import { useId, useMemo, useRef, useState } from 'react';
import clsx from 'clsx';
import { Autocomplete, AutocompleteInput, AutocompleteItem, AutocompleteList, AutocompletePopup } from '@/components/ui/autocomplete';
import { Menu } from '../../presentation/primitives/Menu';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';
import { IconChevronDownOutline14, IconClockOutline16 } from '../../presentation/primitives/icons';
import { DatePicker } from './DatePicker';
import { ClockPicker } from './ClockPicker';
import { IconCalendarOutlineRegular } from './CalendarIcon';
import type { TaskTranslate } from './copy';
import { browserZone, secondPrecision, zoneLabel, type TimingDraft, type TimingKind } from './timing';
import { cronPreview, parseCronExpression } from './cron';
import { taskManagerCss } from './taskManagerCss';
import { schedCss } from './schedCss';

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
  const zoneLabels = useMemo(() => zones.map(zone => zone.label), [zones]);
  const zoneByLabel = useMemo(() => new Map(zones.map(zone => [zone.label, zone])), [zones]);
  return <section className={taskManagerCss.ruleCard} aria-label={t('timing.title')}>
    <div className={taskManagerCss.ruleRows}>
      <div className={taskManagerCss.ruleRow}><span className={taskManagerCss.ruleLabel}>{t('timing.repeat')}</span>
        <Menu open={menu} onClose={() => setMenu(false)} items={choices.map(id => ({ id, label: label(id) }))} selectedId={selected} onSelect={id => select(id as Choice)}
          anchor={<Button size="sm" className={taskManagerCss.ruleCurrent} disabled={disabled} aria-label={t('timing.repeat')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(!menu)}>{label(selected)}<IconChevronDownOutline14/></Button>}/>
      </div>
      {value.kind === 'interval' ? <div className={taskManagerCss.ruleRow}><label className={taskManagerCss.ruleLabel} htmlFor={id + '-quantity'}>{t('timing.number')}</label>
        <div className={taskManagerCss.ruleInterval}><Input id={id + '-quantity'} className={taskManagerCss.ruleIntervalInput} type="number" min={value.unit === 'second' ? 60 : 1} step="any" disabled={disabled} value={value.quantity} onChange={event => update({ quantity: event.target.value })}/>
          <span className={taskManagerCss.ruleIntervalUnit}>{t(value.unit === 'hour' ? 'timing.hours' : value.unit === 'minute' ? 'timing.minutes' : 'timing.seconds')}</span></div>
      </div> : <>
        {value.kind === 'weekly' && <div className={taskManagerCss.ruleRow}><span className={taskManagerCss.ruleLabel}>{t('timing.weekday')}</span><div className={taskManagerCss.ruleWeekdays} role="group" aria-label={t('timing.weekday')}>
          {([1, 2, 3, 4, 5, 6, 7] as const).map(day => <button key={day} type="button" disabled={disabled} aria-pressed={value.weekdays.includes(day)} onClick={() => update({ weekdays: value.weekdays.includes(day) ? value.weekdays.filter(value => value !== day) : [...value.weekdays, day] })}>{t(('frequency.weekday.' + day) as 'frequency.weekday.1')}</button>)}
        </div></div>}
        {value.kind === 'once' && <div className={taskManagerCss.ruleRow}><span className={taskManagerCss.ruleLabel}>{t('timing.date')}</span>
          <button ref={date} type="button" className={clsx(taskManagerCss.pickerTrigger, schedCss.pickerButton)} disabled={disabled} aria-label={t('timing.date')} aria-haspopup="dialog" aria-expanded={picker === 'date'} onClick={() => setPicker(picker === 'date' ? undefined : 'date')}><span>{value.date.replaceAll('-', '/')}</span><IconCalendarOutlineRegular/></button>
          <DatePicker open={picker === 'date'} anchorRef={date} value={value.date} onPick={date => update({ date })} onClose={() => setPicker(undefined)} t={t}/>
        </div>}
        {value.kind !== 'cron' && <div className={taskManagerCss.ruleRow}><span className={taskManagerCss.ruleLabel}>{t('timing.time')}</span>
          <button ref={clock} type="button" className={clsx(taskManagerCss.pickerTrigger, schedCss.pickerButton)} disabled={disabled} aria-label={t('timing.time')} aria-haspopup="dialog" aria-expanded={picker === 'time'} onClick={() => setPicker(picker === 'time' ? undefined : 'time')}><span>{value.kind === 'once' ? secondPrecision(value.time) : value.time.slice(0, 5)}</span><IconClockOutline16/></button>
          <ClockPicker open={picker === 'time'} anchorRef={clock} value={value.time} seconds={value.kind === 'once'} onPick={time => update({ time })} onClose={() => setPicker(undefined)} t={t}/>
        </div>}
        {value.kind === 'cron' && <div className={taskManagerCss.ruleRow}><label className={taskManagerCss.ruleLabel} htmlFor={id + '-cron'}>{t('timing.expression')}</label><div className={taskManagerCss.ruleControl}>
          <Input id={id + '-cron'} className={taskManagerCss.ruleInput} disabled={disabled} value={value.expression} spellCheck={false} onChange={event => update({ expression: event.target.value })}/>
          <p className={parsed ? taskManagerCss.ruleHint : taskManagerCss.ruleHintError}>{parsed ? cronPreview(parsed, t, locale) : t('timing.invalid-cron')}</p>
        </div></div>}
        <div className={taskManagerCss.ruleRow}><span className={taskManagerCss.ruleLabel}>{t('timing.zone')}</span>
          <div className={schedCss.zoneCombo}><Autocomplete items={zoneLabels} filteredItems={zoneQuery === undefined ? zoneLabels : undefined}
            value={zoneQuery ?? zoneLabel(value.zone, locale)} onValueChange={setZoneQuery} openOnInputClick disabled={disabled}>
            <AutocompleteInput aria-label={t('timing.zone')} placeholder={t('timing.zone-search')} showTrigger onBlur={() => setZoneQuery(undefined)} />
            <AutocompletePopup className={schedCss.zonePopup} align="end"><AutocompleteList className={schedCss.zoneList} aria-label={t('timing.zone')}>
              {(label: string, index: number) => {
                const zone = zoneByLabel.get(label)!;
                return <AutocompleteItem key={zone.id} value={zone.label} index={index} className={schedCss.zoneOption}
                  onClick={() => { update({ zone: zone.id }); setZoneQuery(undefined); }}>{zone.label}</AutocompleteItem>;
              }}
            </AutocompleteList></AutocompletePopup>
          </Autocomplete></div>
        </div>
      </>}
    </div>
    {value.kind === 'once' && <p className={taskManagerCss.ruleHint}>{t('timing.timezone-note')}</p>}
  </section>;
}
