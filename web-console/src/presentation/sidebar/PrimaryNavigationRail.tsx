import type { ReactNode } from 'react';
import clsx from 'clsx';
import { Tooltip } from '../primitives/Tooltip';

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
    <button type="button" className={clsx('flex-none flex items-center justify-center w-[40px] h-[40px] p-0 border-0 rounded-[var(--dsw-radius-md)] bg-transparent [color:var(--dsw-alias-label-secondary)] cursor-pointer [-webkit-app-region:no-drag] [&>span]:flex [&>span]:items-center [&>span]:justify-center hover:bg-[var(--dsw-alias-interactive-bg-hover)] hover:[color:var(--dsw-alias-label-primary)] focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:outline-offset-[-2px]', activeId === item.id && 'bg-[var(--dsw-alias-interactive-bg-active)] [color:var(--dsw-alias-label-primary)]')} data-destination={item.id}
      aria-label={item.label} aria-current={activeId === item.id ? 'page' : undefined}
      aria-controls={item.controls} aria-expanded={item.expanded} onClick={item.select}>
      <span aria-hidden="true">{item.icon}</span>
    </button>
  </Tooltip>;
  return <nav aria-label={label} className="flex flex-col h-full box-border pt-[18px] px-[8px] pb-[12px] bg-[var(--dsw-specific-sidebar-fill)] [color:var(--dsw-alias-label-primary)] [[data-platform=darwin]:not([data-fullscreen])_&]:pt-[58px]" data-primary-navigation>
    <div className="flex flex-col items-center gap-[10px]">{items.map(control)}</div>
    <div className="flex flex-col items-center gap-[10px] mt-auto pt-[12px]">{feedback}{control(settings)}</div>
  </nav>;
}
