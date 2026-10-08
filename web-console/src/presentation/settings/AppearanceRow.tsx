/* Copyright (c) 2026 DeepSeek. MIT. Adapted ui-theme AppearanceRow; see PROVENANCE.md. */
import { IconDarkOutline16, IconFollowsystemOutline16, IconLightOutline16 } from '../primitives/icons';
import css from './AppearanceRow.module.css';

const choices = [ ['light', IconLightOutline16], ['dark', IconDarkOutline16], ['system', IconFollowsystemOutline16] ] as const;
export function AppearanceRow({ value, onChange, title, labels }: {
  value: 'light' | 'dark' | 'system'; onChange?: (value: 'light' | 'dark' | 'system') => void;
  title: string; labels: Record<'light' | 'dark' | 'system', string>;
}) {
  return <div className={css.group} role="group" aria-label={title}>
    <div className={css.title}>{title}</div>
    <div className={css.cubeRow}>{choices.map(([id, Icon]) => <button key={id} type="button"
      className={`${css.themeCube}${value === id ? ` ${css.selected}` : ''}`} aria-pressed={value === id}
      onClick={() => onChange?.(id)}><span aria-hidden="true"><Icon/></span>{labels[id]}</button>)}</div>
  </div>;
}
