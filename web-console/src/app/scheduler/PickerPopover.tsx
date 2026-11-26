/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-schedule/PickerPopover; see PROVENANCE.md. */
import type { ReactNode, RefObject } from 'react';
import clsx from 'clsx';
import { Popover, PopoverPopup } from '@/components/ui/popover';

/** Scheduler picker surface backed by coss ui/Base UI positioning and dismissal. */
export function PickerPopover({ open, anchorRef, label, className, onClose, children }: {
  open: boolean; anchorRef: RefObject<HTMLElement | null>; label: string; className?: string;
  onClose: () => void; children: ReactNode;
}) {
  return <Popover open={open} onOpenChange={next => { if (!next) onClose(); }}>
    <PopoverPopup
      anchor={anchorRef}
      positionMethod="fixed"
      collisionPadding={12}
      side="bottom"
      align="start"
      sideOffset={4}
      className={clsx('box-border z-[1100] flex border-0 rounded-[var(--dsw-radius-lg)] bg-[var(--dsw-specific-menu)] [backdrop-filter:var(--dsw-menu-backdrop-filter)] [--dsw-elevation-stroke-color:var(--dsw-alias-border-l1)] shadow-[var(--dsw-elevation-prominent)] [--dsh-scrollbar-thumb:var(--dsw-alias-scrollbar-bg-l2)] [--dsh-scrollbar-thumb-hover:var(--dsw-alias-scrollbar-hover-l2)] [&>[data-slot=popover-viewport]]:p-0', className)}
      role="dialog"
      aria-label={label}
      finalFocus={() => anchorRef.current ?? false}
    >
      {children}
    </PopoverPopup>
  </Popover>;
}
