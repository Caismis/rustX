/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useRef, type ReactNode } from 'react'
import clsx from 'clsx'
import { DialogSurface } from './DialogSurface'
import { IconCloseOutline16 } from './icons/index.tsx'
import css from './Modal.module.css'

interface ModalBaseProps {
  open: boolean
  onClose: () => void
  title: string
  description?: string
  children?: ReactNode
  footer?: ReactNode
  className?: string
  contentClassName?: string
  /* Optional class for the fixed full-viewport layer — e.g. a dialog that
   * must sit above another dialog raises its stacking level here. */
  rootClassName?: string
  finalFocus?: () => HTMLElement | false
}

type ModalProps = ModalBaseProps & (
  | { headless: true; closeLabel?: never }
  | { headless?: false; closeLabel: string }
)

/**
 * Render a centered, body-portaled modal over a blurred page mask. coss ui's
 * Base UI dialog primitive owns modality, focus containment, dismissal and
 * focus restoration; rustX keeps the card geometry and content contract.
 * @returns null when closed; otherwise the overlay tree.
 */
export function Modal({
  open, onClose, title, closeLabel, description, children, footer, className, contentClassName, rootClassName, headless = false, finalFocus,
}: ModalProps) {
  const popup = useRef<HTMLElement>(null)
  if (!open) return null
  return (
    <DialogSurface
      open
      onClose={onClose}
      title={title}
      overlayClassName={clsx(css.root, css.scrim, rootClassName)}
      className={clsx(css.dialog, className)}
      contentRef={popup}
      finalFocus={finalFocus}
      initialFocus={() => popup.current?.querySelector<HTMLElement>('[data-modal-autofocus], [autofocus]') ?? true}
    >
      {headless
        ? children
        : (
          <>
            <div className={clsx(css.content, contentClassName)}>
              <div className={css.header}>
                <h2 className={css.title}>{title}</h2>
                <button type="button" className={css.close} aria-label={closeLabel} onClick={onClose}>
                  <IconCloseOutline16 size={14} />
                </button>
              </div>
              {description !== undefined && description !== '' && (
                <p className={css.description}>{description}</p>
              )}
              {children !== undefined && <div className={css.body}>{children}</div>}
            </div>
            {footer !== undefined && <div className={css.footer}>{footer}</div>}
          </>
        )}
    </DialogSurface>
  )
}
