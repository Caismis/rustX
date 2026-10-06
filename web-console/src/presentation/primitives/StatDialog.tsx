/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat stat-dialog.ts and the DialogPill /
// TurnUsagePanel / ContextMeter panels: one trigger-anchored stat dialog above
// its pill, kept 12px inside the viewport and closed by Escape or an outside
// press. Base UI Popover owns the placement and dismissal that Harness
// hand-rolls with useAnchoredPosition and useDismissOnOutsidePointer.
import { Popover } from '@base-ui/react/popover';
import type { ReactNode } from 'react';
import css from './StatDialog.module.css';

export { css as statCss };

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
  return <Popover.Root>
    <Popover.Trigger className={triggerClassName} aria-label={label}>{trigger}</Popover.Trigger>
    <Popover.Portal>
      <Popover.Positioner className={css.positioner} side="top" sideOffset={8} collisionPadding={12}>
        <Popover.Popup className={panelClassName ?? css.panel} aria-label={title}>{children}</Popover.Popup>
      </Popover.Positioner>
    </Popover.Portal>
  </Popover.Root>;
}

/** The dialog heading: glyph and section name left, the headline value right, over a rule. */
export function StatHeading({ icon, title, value }: { icon: ReactNode; title: string; value?: string }) {
  return <>
    <div className={css.title}>
      <span className={css.titleLabel}>{icon}{title}</span>
      {value !== undefined && <span className={css.titleValue}>{value}</span>}
    </div>
    <div className={css.titleRule} aria-hidden="true"/>
  </>;
}
