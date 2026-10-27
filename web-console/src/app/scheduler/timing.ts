import { parseAbsolute, parseDateTime, toCalendarDateTime, toZoned } from '@internationalized/date';
import type { ScheduleSpec, ScheduleTiming } from '../../../../protocol/app-server/v61';
import { cronPreview, parseCronExpression } from './cron';
import type { TaskTranslate } from './copy';

export type TimingKind = 'once' | 'interval' | 'daily' | 'weekdays' | 'weekly' | 'cron';
export type IntervalUnit = 'second' | 'minute' | 'hour';
export interface TimingDraft {
  kind: TimingKind; date: string; time: string; zone: string; quantity: string; unit: IntervalUnit;
  weekdays: number[]; expression: string;
}
export const browserZone = () => Intl.DateTimeFormat().resolvedOptions().timeZone;
export const secondPrecision = (value: string) => value.split('.')[0].length === 5 ? value.split('.')[0] + ':00' : value.split('.')[0];
export function timingDraft(spec: ScheduleSpec, zone = browserZone()): TimingDraft {
  const instant = spec.type === 'at' ? spec.at_utc : spec.type === 'interval' ? spec.anchor_utc : new Date().toISOString();
  const wall = parseAbsolute(instant, spec.type === 'cron' ? spec.time_zone : zone);
  const local = toCalendarDateTime(wall).toString();
  const [date, time] = local.split('T');
  const base: TimingDraft = { kind: 'once', date, time, zone: wall.timeZone, quantity: '1', unit: 'hour', weekdays: [1], expression: '0 9 * * 1-5' };
  if (spec.type === 'interval') {
    const unit = spec.every_seconds % 3600 === 0 ? 'hour' : spec.every_seconds % 60 === 0 ? 'minute' : 'second';
    return { ...base, kind: 'interval', quantity: String(spec.every_seconds / (unit === 'hour' ? 3600 : unit === 'minute' ? 60 : 1)), unit };
  }
  if (spec.type === 'cron') {
    const parsed = parseCronExpression(spec.expression);
    const fields = spec.expression.trim().split(/\s+/);
    const simple = parsed && parsed.minutes.values.length === 1 && parsed.hours.values.length === 1 && fields[2] === '*' && fields[3] === '*';
    if (simple) {
      const at = String(parsed.hours.values[0]).padStart(2, '0') + ':' + String(parsed.minutes.values[0]).padStart(2, '0') + ':00';
      const days = parsed.daysOfWeek.values.map(day => day === 0 ? 7 : day).sort((a, b) => a - b);
      const kind = fields[4] === '*' ? 'daily' : days.join(',') === '1,2,3,4,5' ? 'weekdays' : 'weekly';
      return { ...base, kind, time: at, weekdays: days, expression: spec.expression };
    }
    return { ...base, kind: 'cron', expression: spec.expression };
  }
  return base;
}
export class TimingInputError extends Error {
  constructor(readonly key: 'timing.invalid' | 'timing.invalid-zone' | 'timing.gap' | 'timing.minimum' | 'timing.invalid-cron' | 'timing.minute-precision') { super(key); }
}
export function timingValue(draft: TimingDraft, now = Date.now()): ScheduleTiming {
  if (draft.kind === 'interval') {
    const seconds = Number(draft.quantity) * (draft.unit === 'hour' ? 3600 : draft.unit === 'minute' ? 60 : 1);
    if (!Number.isInteger(seconds) || seconds < 60 || seconds > 4_294_967_295) throw new TimingInputError('timing.minimum');
    return { type: 'interval', every_seconds: seconds };
  }
  try { new Intl.DateTimeFormat('en', { timeZone: draft.zone }); } catch { throw new TimingInputError('timing.invalid-zone'); }
  if (draft.kind === 'once') {
    let wall;
    try { wall = parseDateTime(draft.date + 'T' + draft.time); } catch { throw new TimingInputError('timing.invalid'); }
    const zoned = toZoned(wall, draft.zone, 'earlier');
    if (toCalendarDateTime(zoned).compare(wall) !== 0) throw new TimingInputError('timing.gap');
    const at = zoned.toAbsoluteString();
    if (Date.parse(at) <= now) throw new TimingInputError('timing.invalid');
    return { type: 'at', at };
  }
  let expression = draft.expression;
  if (draft.kind !== 'cron') {
    const match = /^(\d\d):(\d\d)(?::(\d\d)(?:\.(\d+))?)?$/.exec(draft.time);
    if (!match || Number(match[1]) > 23 || Number(match[2]) > 59) throw new TimingInputError('timing.invalid');
    if (Number(match[3] ?? 0) !== 0 || Number(match[4] ?? 0) !== 0) throw new TimingInputError('timing.minute-precision');
    const days = draft.kind === 'daily' ? '*' : draft.kind === 'weekdays' ? '1-5' : [...new Set(draft.weekdays)].sort((a, b) => a - b).join(',');
    expression = Number(match[2]) + ' ' + Number(match[1]) + ' * * ' + days;
  }
  if (!parseCronExpression(expression)) throw new TimingInputError('timing.invalid-cron');
  return { type: 'cron', expression, time_zone: draft.zone };
}
export function sameTiming(a: TimingDraft, b: TimingDraft) {
  return a.kind === b.kind && (a.kind === 'interval'
    ? Number(a.quantity) * (a.unit === 'hour' ? 3600 : a.unit === 'minute' ? 60 : 1) === Number(b.quantity) * (b.unit === 'hour' ? 3600 : b.unit === 'minute' ? 60 : 1)
    : a.kind === 'once' ? a.date === b.date && a.time === b.time && a.zone === b.zone
      : a.kind === 'cron' ? a.expression === b.expression && a.zone === b.zone
        : a.time === b.time && a.zone === b.zone && (a.kind !== 'weekly' || [...a.weekdays].sort().join(',') === [...b.weekdays].sort().join(',')));
}
export function displayInstant(value: string, locale: string, now = Date.now()) {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return { absolute: value, relative: '' };
  const difference = date.getTime() - now;
  const magnitude = Math.abs(difference);
  const unit: Intl.RelativeTimeFormatUnit = magnitude >= 86_400_000 ? 'day' : magnitude >= 3_600_000 ? 'hour' : magnitude >= 60_000 ? 'minute' : 'second';
  const scale = unit === 'day' ? 86_400_000 : unit === 'hour' ? 3_600_000 : unit === 'minute' ? 60_000 : 1000;
  return { absolute: new Intl.DateTimeFormat(locale, { year: date.getFullYear() === new Date(now).getFullYear() ? undefined : 'numeric', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(date),
    relative: new Intl.RelativeTimeFormat(locale, { numeric: 'auto' }).format(Math.round(difference / scale), unit) };
}
export function zoneLabel(zone: string, locale: string) {
  const date = new Date();
  try {
    const offset = new Intl.DateTimeFormat('en', { timeZone: zone, timeZoneName: 'longOffset' }).formatToParts(date).find(part => part.type === 'timeZoneName')?.value.replace('GMT', 'UTC') ?? 'UTC';
    const name = new Intl.DateTimeFormat(locale, { timeZone: zone, timeZoneName: 'longGeneric' }).formatToParts(date).find(part => part.type === 'timeZoneName')?.value ?? zone;
    return offset + ' · ' + name;
  } catch { return zone; }
}
export function frequency(spec: ScheduleSpec, t: TaskTranslate, locale: string) {
  if (spec.type === 'at') return t('frequency.once');
  if (spec.type === 'interval') {
    const draft = timingDraft(spec);
    const count = Number(draft.quantity);
    return t('frequency.every', { interval: new Intl.NumberFormat(locale, { style: 'unit', unit: draft.unit, unitDisplay: 'long' }).format(count) });
  }
  const parsed = parseCronExpression(spec.expression);
  const description = parsed ? cronPreview(parsed, t, locale) : spec.expression;
  return spec.time_zone === browserZone() ? description : description + ' · ' + zoneLabel(spec.time_zone, locale);
}
