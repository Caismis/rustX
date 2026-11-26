/* Copyright (c) 2026 DeepSeek. MIT. Adapted ui-theme AppearanceRow; see PROVENANCE.md. */
import { IconDarkOutline16, IconFollowsystemOutline16, IconLightOutline16 } from '../primitives/icons';

const choices = [ ['light', IconLightOutline16], ['dark', IconDarkOutline16], ['system', IconFollowsystemOutline16] ] as const;
export function AppearanceRow({ value, onChange, title, labels }: {
  value: 'light' | 'dark' | 'system'; onChange?: (value: 'light' | 'dark' | 'system') => void;
  title: string; labels: Record<'light' | 'dark' | 'system', string>;
}) {
  return <div className="flex flex-col gap-[8px] px-0 py-[16px] [border-bottom:0.5px_solid_var(--dsw-alias-border-l2)]" role="group" aria-label={title}>
    <div className="text-[14px] font-normal leading-[22px] [color:var(--dsw-alias-label-primary)]">{title}</div>
    <div className="grid grid-cols-3 items-stretch gap-[8px]">{choices.map(([id, Icon]) => <button key={id} type="button"
      className={`box-border min-w-0 flex flex-col items-center justify-center gap-[4px] px-[8px] py-[20px] [border:0.5px_solid_var(--dsw-alias-border-l4)] rounded-[var(--dsw-radius-xl)] bg-transparent [font:inherit] text-[14px] leading-[22px] [color:var(--dsw-alias-label-primary)] cursor-pointer [&>span]:flex${value === id ? ' bg-[var(--dsw-alias-bg-module-platform)]! [border-color:var(--dsw-static-neutral-bluish-400)]!' : ' hover:bg-[var(--dsw-alias-interactive-bg-hover)]'}`} aria-pressed={value === id}
      onClick={() => onChange?.(id)}><span aria-hidden="true"><Icon/></span>{labels[id]}</button>)}</div>
  </div>;
}
