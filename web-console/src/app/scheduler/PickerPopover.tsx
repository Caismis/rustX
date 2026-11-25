/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-schedule/PickerPopover; see PROVENANCE.md. */
import type { ReactNode, RefObject } from 'react';
import clsx from 'clsx';
import { Popover, PopoverPopup } from '@/components/ui/popover';
import css from './PickerPopover.module.css';

/** Scheduler picker surface backed by coss ui/Base UI positioning and dismissal. */
export function PickerPopover({ open, anchorRef, label, className, onClose, children }: {
  open: boolean; anchorRef: RefObject<HTMLElement | null>; label: string; className?: string;
  onClose: () => void; children: ReactNode;
}) {
  return <Popover open={open} onOpenChange={next => { if (!next) onClose(); }}>
    <PopoverPopup
      anchor={anchorRef}
      side="bottom"
      align="start"
      sideOffset={4}
      className={clsx(css.panel, className)}
      role="dialog"
      aria-label={label}
      finalFocus={() => anchorRef.current ?? false}
    >
      {children}
    </PopoverPopup>
  </Popover>;
}
