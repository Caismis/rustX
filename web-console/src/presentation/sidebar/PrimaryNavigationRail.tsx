import type { ReactNode } from 'react';
import clsx from 'clsx';
import { Tooltip } from '../primitives/Tooltip';
import css from './PrimaryNavigationRail.module.css';

export interface PrimaryNavigationItem {
  id: string; label: string; icon: ReactNode; select: () => void;
  controls?: string; expanded?: boolean;
}

/** Permanent, presentation-only navigation. Selection comes from the committed route. */
export function PrimaryNavigationRail({ label, items, settings, activeId, feedback }: {
  label: string; items: readonly PrimaryNavigationItem[]; settings: PrimaryNavigationItem;
  activeId: string; feedback?: ReactNode;
}) {
  const control = (item: PrimaryNavigationItem) => <Tooltip key={item.id} label={item.label} delayMs={300}>
    <button type="button" className={clsx(css.item, activeId === item.id && css.active)} data-destination={item.id}
      aria-label={item.label} aria-current={activeId === item.id ? 'page' : undefined}
      aria-controls={item.controls} aria-expanded={item.expanded} onClick={item.select}>
      <span aria-hidden="true">{item.icon}</span>
    </button>
  </Tooltip>;
  return <nav aria-label={label} className={css.rail} data-primary-navigation>
    <div className={css.destinations}>{items.map(control)}</div>
    <div className={css.bottom}>{feedback}{control(settings)}</div>
  </nav>;
}
