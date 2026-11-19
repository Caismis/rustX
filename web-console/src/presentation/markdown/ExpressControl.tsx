import { useId, useState, type ChangeEvent, type CSSProperties, type ReactNode } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../locale/react';
import css from './ExpressControl.module.css';

export interface ExpressControlProps {
  kind: string;
  props: Record<string, unknown>;
  onChange: (value: string | number | boolean | string[]) => void;
  children?: ReactNode;
}

function text(value: unknown): string | undefined {
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return undefined;
}

function num(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function bool(value: unknown): boolean | undefined {
  return typeof value === 'boolean' ? value : undefined;
}

function stringList(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : [];
}

function choiceOptions(value: unknown): { label: string; value: string }[] {
  if (!Array.isArray(value)) return [];
  const out: { label: string; value: string }[] = [];
  for (const item of value) {
    if (typeof item !== 'object' || item === null) continue;
    const record = item as Record<string, unknown>;
    const label = text(record.label);
    const optionValue = text(record.value);
    if (label !== undefined && optionValue !== undefined) out.push({ label, value: optionValue });
  }
  return out;
}

function ControlButton({ props, children }: { props: Record<string, unknown>; children?: ReactNode }) {
  const variant = text(props.variant);
  const appearance = props.color === 'danger' ? 'destructive'
    : variant === 'primary' || variant === 'solid' ? 'default'
      : variant === 'soft' ? 'secondary'
        : variant === 'borderless' || variant === 'ghost' ? 'ghost' : 'outline';
  return (
    <button type="button" disabled data-slot="button" data-variant={appearance}
      data-size={props.size === 'sm' || props.size === 'xs' ? 'sm' : 'lg'}
      className={clsx(css.button, props.block === true && css.fullWidth)}>
      {children}
    </button>
  );
}

function ControlSlider({ props, onChange }: { props: Record<string, unknown>; onChange: ExpressControlProps['onChange'] }) {
  const label = text(props.label);
  const min = num(props.min) ?? 0;
  const max = Math.max(num(props.max) ?? 100, min);
  const value = Math.min(max, Math.max(min, num(props.value) ?? min));
  const divisions = num(props.steps);
  const step = divisions !== undefined && divisions > 0 && max > min ? (max - min) / divisions : 'any';
  const fill = max > min ? (value - min) / (max - min) : 0;
  return (
    <div className={css.control}>
      {label !== undefined && <span className={css.controlLabel}>{label}</span>}
      <span className={css.slider}>
        <input type="range" data-slot="slider" aria-label={label} min={min} max={max} step={step} value={value}
          style={{ '--a2-slider-fill': `${(Number.isFinite(fill) ? fill : 0) * 100}%` } as CSSProperties}
          onChange={event => onChange(Number(event.target.value))} />
        <output className={css.sliderValue}>{value}</output>
      </span>
    </div>
  );
}

function ControlCheckBox({ props, onChange }: { props: Record<string, unknown>; onChange: ExpressControlProps['onChange'] }) {
  return (
    <label className={css.check}>
      <input type="checkbox" data-slot="checkbox" checked={bool(props.value) ?? false}
        onChange={event => onChange(event.target.checked)} />
      <span>{text(props.label) ?? ''}</span>
    </label>
  );
}

function ControlTextField({ props, onChange }: { props: Record<string, unknown>; onChange: ExpressControlProps['onChange'] }) {
  const variant = text(props.variant) ?? 'shortText';
  const shared = {
    value: text(props.value) ?? '',
    placeholder: text(props.placeholder),
    onChange: (event: ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) => onChange(event.target.value),
  };
  return (
    <label className={css.control}>
      <span className={css.controlLabel}>{text(props.label) ?? ''}</span>
      {variant === 'longText'
        ? <textarea data-slot="textarea" className={css.field} rows={3} {...shared} />
        : <input data-slot="input" className={css.field}
            type={variant === 'number' ? 'number' : variant === 'obscured' ? 'password' : 'text'}
            {...shared} />}
    </label>
  );
}

function inputValue(type: string, value: string): string {
  if (type === 'date') return /^(\d{4}-\d{2}-\d{2})/.exec(value)?.[1] ?? '';
  if (type === 'time') return /(\d{2}:\d{2})/.exec(value)?.[1] ?? '';
  if (type === 'datetime-local') return /^(\d{4}-\d{2}-\d{2})[T ](\d{2}:\d{2})/.exec(value)?.[0].replace(' ', 'T') ?? '';
  return value;
}

function ControlDateTime({ props, onChange }: { props: Record<string, unknown>; onChange: ExpressControlProps['onChange'] }) {
  const enableDate = bool(props.enableDate) ?? false;
  const enableTime = bool(props.enableTime) ?? false;
  const type = enableDate && enableTime ? 'datetime-local' : enableDate ? 'date' : enableTime ? 'time' : 'text';
  const bound = (value: unknown) => {
    const raw = text(value);
    return raw === undefined ? undefined : inputValue(type, raw);
  };
  return (
    <label className={css.control}>
      {text(props.label) !== undefined && <span className={css.controlLabel}>{text(props.label)}</span>}
      <input data-slot="input" className={css.field} type={type} value={inputValue(type, text(props.value) ?? '')}
        min={bound(props.min)} max={bound(props.max)}
        onChange={event => onChange(event.target.value)} />
    </label>
  );
}

function ControlChoicePicker({ props, onChange }: { props: Record<string, unknown>; onChange: ExpressControlProps['onChange'] }) {
  const tx = useTranslation();
  const group = useId();
  const [filter, setFilter] = useState('');
  const multiple = text(props.variant) === 'multipleSelection';
  const selected = stringList(props.value);
  const options = choiceOptions(props.options);
  const query = filter.trim().toLowerCase();
  const visible = query === '' ? options : options.filter(option => option.label.toLowerCase().includes(query));
  const toggle = (value: string) => {
    onChange(multiple
      ? selected.includes(value) ? selected.filter(item => item !== value) : [...selected, value]
      : selected[0] === value ? [] : [value]);
  };
  const label = text(props.label);
  return (
    <div className={css.control}>
      {label !== undefined && <span id={`${group}-label`} className={css.controlLabel}>{label}</span>}
      {bool(props.filterable) === true && (
        <input data-slot="input" className={css.field} type="search" value={filter} aria-label={tx('interactions:express.filter-options')}
          onChange={event => setFilter(event.target.value)} />
      )}
      {text(props.displayStyle) === 'chips' ? (
        <div className={css.chips} role="group" aria-labelledby={label ? `${group}-label` : undefined}>
          {visible.map(option => {
            const active = selected.includes(option.value);
            return (
              <button key={option.value} type="button" aria-pressed={active}
                className={css.chip} data-slot="toggle"
                onClick={() => toggle(option.value)}>
                {option.label}
              </button>
            );
          })}
        </div>
      ) : (
        <div className={css.choiceList} role={multiple ? 'group' : 'radiogroup'} aria-labelledby={label ? `${group}-label` : undefined}>
          {visible.map(option => (
            <label key={option.value} className={css.choice}>
              <input type={multiple ? 'checkbox' : 'radio'} name={group}
                checked={selected.includes(option.value)}
                onChange={() => toggle(option.value)} />
              <span>{option.label}</span>
            </label>
          ))}
        </div>
      )}
    </div>
  );
}

export function ExpressControl({ kind, props, onChange, children }: ExpressControlProps): ReactNode {
  switch (kind) {
    case 'Button': return <ControlButton props={props}>{children}</ControlButton>;
    case 'Slider': return <ControlSlider props={props} onChange={onChange} />;
    case 'CheckBox': return <ControlCheckBox props={props} onChange={onChange} />;
    case 'TextField': return <ControlTextField props={props} onChange={onChange} />;
    case 'ChoicePicker': return <ControlChoicePicker props={props} onChange={onChange} />;
    case 'DateTimeInput': return <ControlDateTime props={props} onChange={onChange} />;
    default: return null;
  }
}
