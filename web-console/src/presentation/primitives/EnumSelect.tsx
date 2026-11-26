/** Shared closed-string select backed by coss ui/Base UI. */
import type { ComponentPropsWithoutRef, ReactNode } from 'react';
import {
  Select,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

export function EnumSelect<T extends string>({ label, value, options, onChange, disabled = false, placeholder, className, id, ...triggerProps }: {
  label: string;
  value: T | null;
  options: readonly { value: T; label: ReactNode; disabled?: boolean }[];
  onChange: (value: T) => void;
  disabled?: boolean;
  placeholder?: string;
  className?: string;
  id?: string;
} & Omit<ComponentPropsWithoutRef<'button'>, 'id' | 'className' | 'disabled' | 'value' | 'onChange' | 'children'>) {
  return <Select
    items={options.map(option => ({ value: option.value, label: option.label }))}
    value={value}
    disabled={disabled}
    onValueChange={next => { if (next !== null) onChange(next as T); }}
  >
    <SelectTrigger id={id} aria-label={label} className={className} {...triggerProps}>
      <SelectValue placeholder={placeholder} />
    </SelectTrigger>
    <SelectPopup>
      {options.map(option => <SelectItem key={option.value} value={option.value} disabled={option.disabled}>{option.label}</SelectItem>)}
    </SelectPopup>
  </Select>;
}
