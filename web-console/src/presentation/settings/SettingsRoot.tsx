/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { Fragment, useId, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useModalFocus } from '../primitives/useModalFocus';
import clsx from 'clsx';
import { IconCloseOutline16, IconSettingsOutline16 } from '../primitives/icons';
import css from './SettingsRoot.module.css';
/**
 * The modal layer: full-viewport mask + centered panel. Close paths: the
 * header button, a mask click, and document-level Escape (mounted only while
 * open, so the listener lifetime is the panel's).
 */
export function SettingsPanel({ rows, activeId, onSelect, onClose, children, actions }: {
  rows: readonly { id: string; label: string; group?: string }[]; activeId: string; onSelect: (id: string) => void;
  onClose: () => void; children: ReactNode; actions?: ReactNode;
}) {
  // Entries can unmount underneath the requested id, so the render-time
  // projection falls back to the first row when the id is gone.
  const active = rows.find(r => r.id === activeId)?.id ?? rows[0]?.id
  const titleId = useId();
  const dialog = useRef<HTMLDivElement>(null);
  useModalFocus(true, dialog, onClose);



  // Entering the dialog focuses the close button; the root restores its trigger on close.
  const closeButton = useRef<HTMLButtonElement | null>(null)

  return createPortal(
    <div className={css.overlay} role="presentation">
      <div className={css.mask} aria-hidden="true" onClick={onClose} />
      <div ref={dialog} className={css.panel} role="dialog" aria-modal="true" aria-labelledby={titleId}>
        <nav className={css.nav} aria-label="Settings sections">
          <div className={css.navTitle} id={titleId}>Settings</div>
          <div className={css.navList}>
            {rows.map((row, index) => (
              <Fragment key={row.id}>{row.group && row.group !== rows[index - 1]?.group && <span className={css.group}>{row.group}</span>}<button
                key={row.id}
                type="button"
                className={clsx(css.navCell, row.id === active && css.active)}
                aria-current={row.id === active ? 'page' : undefined}
                onClick={() => { onSelect(row.id) }}
              >
                <IconSettingsOutline16 className={css.navIcon} size={16} />
                <span className={css.navLabel}>{row.label}</span>
              </button></Fragment>
            ))}
          </div>
        </nav>
        <div className={css.content}>
          <div className={css.header}>
            <div className={css.actions}>{actions}</div>
            <button ref={closeButton} type="button" className={css.close} onClick={onClose}>
              <IconCloseOutline16 size={14} />
              <span className={css.hiddenLabel}>Close Settings</span>
            </button>
          </div>
          <div className={css.options}>
            {children}
          </div>
        </div>
      </div>
    </div>, document.body
  )
}


export function SettingsTrigger({ wide, onClick }: { wide: boolean; onClick: () => void }) {
  return <div className={clsx(css.triggerRow, !wide && css.railRow)}><button type="button" className={clsx(css.trigger, !wide && css.rail)} aria-label="Settings" aria-haspopup="dialog" onClick={onClick}>
    <IconSettingsOutline16 />{wide && <span className={css.triggerLabel}>Settings</span>}
  </button></div>;
}
