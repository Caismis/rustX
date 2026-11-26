/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat stat-dialog.ts and the DialogPill /
// TurnUsagePanel / ContextMeter panels: one trigger-anchored stat dialog above
// its pill, kept 12px inside the viewport and closed by Escape or an outside
// press. Base UI Popover owns the placement and dismissal that Harness
// hand-rolls with useAnchoredPosition and useDismissOnOutsidePointer.
import { Popover, PopoverPopup, PopoverTrigger } from '@/components/ui/popover';
import type { ReactNode } from 'react';

const panel = 'box-border w-max min-w-[min(300px,calc(100vw-24px))] max-w-[min(440px,calc(100vw-24px))] p-[16px] border-0 rounded-[16px] bg-[var(--dsw-specific-menu)] [backdrop-filter:blur(40px)_saturate(150%)] [--dsw-elevation-stroke-color:var(--dsw-alias-border-l1)] shadow-[var(--dsw-elevation-prominent)] text-[12px] leading-[18px] [color:var(--dsw-alias-label-secondary)] cursor-default outline-none [&>[data-slot=popover-viewport]]:p-0'
export const statCss = {
  details: 'grid grid-cols-[minmax(76px,auto)_minmax(0,1fr)] gap-x-[16px] gap-y-[6px] m-0 [color:var(--dsw-alias-label-tertiary)] [&_dt]:min-w-0 [&_dt]:m-0 [&_dd]:min-w-0 [&_dd]:m-0 [&_dd]:[color:var(--dsw-alias-label-secondary)] [&_dd]:tabular-nums [&_dd]:text-right',
  route: 'wrap-anywhere',
  reasoning: '[color:var(--dsw-alias-label-tertiary)] whitespace-nowrap',
}


/**
 * A pill button opening its own non-modal stat dialog.
 * @param props.trigger - the pill's visible content.
 * @param props.triggerClassName - the pill skin.
 * @param props.label - the pill's accessible name when its text abbreviates it.
 * @param props.title - the dialog's accessible name.
 * @param props.panelClassName - a panel skin replacing the shared one.
 * @param props.children - the dialog body.
 */
export function StatDialog({ trigger, triggerClassName, label, title, panelClassName, children }: {
  trigger: ReactNode; triggerClassName: string; label?: string; title: string; panelClassName?: string; children: ReactNode;
}) {
  return <Popover>
    <PopoverTrigger className={triggerClassName} aria-label={label}>{trigger}</PopoverTrigger>
    <PopoverPopup side="top" sideOffset={8} collisionPadding={12} className={panelClassName ?? panel} aria-label={title}>{children}</PopoverPopup>
  </Popover>;
}

/** The dialog heading: glyph and section name left, the headline value right, over a rule. */
export function StatHeading({ icon, title, value }: { icon: ReactNode; title: string; value?: string }) {
  return <>
    <div className="flex justify-between gap-[16px] mb-[8px] [color:var(--dsw-alias-label-primary)] font-medium">
      <span className="inline-flex items-center gap-[6px] min-w-0 [&_svg]:w-[14px] [&_svg]:h-[14px] [&_svg]:flex-none">{icon}{title}</span>
      {value !== undefined && <span className="tabular-nums">{value}</span>}
    </div>
    <div className="mb-[10px] [border-top:0.5px_solid_var(--dsw-alias-border-l2)]" aria-hidden="true"/>
  </>;
}
