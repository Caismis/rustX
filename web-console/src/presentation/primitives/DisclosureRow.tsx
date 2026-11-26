/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
import { useId, type KeyboardEvent, type MouseEvent, type ReactNode } from 'react'
import clsx from 'clsx'
import { IconChevronDownOutline14, IconChevronUpOutline14 } from './icons/index.tsx'
import { TextShimmer } from './TextShimmer'

/** Shared 24px disclosure chrome for compact flow rows. */
export interface DisclosureRowProps {
  icon: ReactNode
  title: string
  open: boolean
  expandable: boolean
  onToggle: () => void
  /** Animate only header text while its owning operation is running. */
  running?: boolean
  /** Makes the complete title row the disclosure target. */
  expandOnRowClick?: boolean | undefined
  /** Replaces the collapsed icon with a chevron while the row is hovered. */
  previewChevron?: boolean | undefined
  /** Keeps `collapsedContent` inline while open. */
  keepContentWhenOpen?: boolean | undefined
  collapsedContent?: ReactNode
  children?: ReactNode
  className?: string | undefined
  rowClassName?: string | undefined
  /** Header text sizing and shared base/shimmer layout, beside the leading icon. */
  contentClassName?: string | undefined
  contentLayoutClassName?: string | undefined
  leadingClassName?: string | undefined
  chevronClassName?: string | undefined
  titleClassName?: string | undefined
}

/**
 * Render one disclosure header and its controlled expanded content.
 * @param props - Visual content, controlled state, and interaction policy.
 * @returns the disclosure row.
 */
export function DisclosureRow({
  icon,
  title,
  open,
  expandable,
  onToggle,
  running = false,
  expandOnRowClick = false,
  previewChevron = expandable,
  keepContentWhenOpen = false,
  collapsedContent,
  children,
  className,
  rowClassName,
  contentClassName,
  contentLayoutClassName,
  leadingClassName,
  chevronClassName,
  titleClassName,
}: DisclosureRowProps) {
  const contentId = useId()
  const rowExpands = expandable && expandOnRowClick
  const toggleFromLeading = (event: MouseEvent<HTMLButtonElement>) => {
    event.stopPropagation()
    onToggle()
  }
  const toggleFromKeyboard = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!rowExpands || (event.key !== 'Enter' && event.key !== ' ')) return
    event.preventDefault()
    onToggle()
  }
  const collapsedLeading = previewChevron
    ? (
      <>
        <span className="inline-flex opacity-100 transition-opacity duration-100 group-focus-visible/dr:opacity-0 group-hover/dr:opacity-0">{icon}</span>
        <IconChevronDownOutline14 className={clsx(chevronClassName, 'absolute inset-0 m-auto opacity-0 transition-opacity duration-100 group-focus-visible/dr:opacity-100 group-hover/dr:opacity-100')} />
      </>
    )
    : icon
  const leading = open
    ? <IconChevronUpOutline14 className={chevronClassName} />
    : collapsedLeading

  return (
    <div className={clsx('flex flex-col w-full min-w-0', className)} data-open={open || undefined}>
      <div className="flex items-center min-w-0 data-[has-accessory]:gap-[8px] data-[has-accessory]:[&_[data-disclosure-row]]:flex-[0_1_auto]" data-disclosure-header>
      <div
        className={clsx('group/dr flex-auto relative overflow-hidden flex items-center h-[calc(24px+var(--dsh-content-font-delta,0px))] min-w-0 [color:var(--dsw-alias-label-tertiary)] transition-colors duration-100 motion-reduce:transition-none data-[expandable]:cursor-pointer focus-visible:[color:var(--dsw-alias-label-secondary)] focus-visible:[outline:2px_solid_var(--dsw-alias-button-info-fill)] focus-visible:-outline-offset-2 focus-visible:rounded-[var(--dsw-radius-sm)] hover:[color:var(--dsw-alias-label-secondary)]', rowClassName)}
        data-disclosure-row
        data-expandable={rowExpands || undefined}
        role={rowExpands ? 'button' : undefined}
        tabIndex={rowExpands ? 0 : undefined}
        aria-expanded={rowExpands ? open : undefined}
        aria-controls={rowExpands ? contentId : undefined}
        onClick={rowExpands ? onToggle : undefined}
        onKeyDown={rowExpands ? toggleFromKeyboard : undefined}
      >
        {expandable && !rowExpands ? (
          <button
            type="button"
            className={clsx('relative flex-none w-[calc(16px+var(--dsh-content-font-delta,0px))] h-[calc(16px+var(--dsh-content-font-delta,0px))] inline-flex items-center justify-center mr-[6px] p-0 border-none bg-none [color:inherit] [&_svg:not([data-state])]:w-[calc(14px+var(--dsh-content-font-delta,0px))] [&_svg:not([data-state])]:h-[calc(14px+var(--dsh-content-font-delta,0px))] [&:is(button)]:cursor-pointer', leadingClassName)}
            aria-expanded={open}
            aria-label={title}
            aria-controls={contentId}
            onClick={toggleFromLeading}
          >
            {leading}
          </button>
        ) : (
          <span className={clsx('relative flex-none w-[calc(16px+var(--dsh-content-font-delta,0px))] h-[calc(16px+var(--dsh-content-font-delta,0px))] inline-flex items-center justify-center mr-[6px] p-0 border-none bg-none [color:inherit] [&_svg:not([data-state])]:w-[calc(14px+var(--dsh-content-font-delta,0px))] [&_svg:not([data-state])]:h-[calc(14px+var(--dsh-content-font-delta,0px))] [&:is(button)]:cursor-pointer', leadingClassName)}>
            {leading}
          </span>
        )}
        <TextShimmer active={running} className={clsx('flex-auto min-w-0', contentClassName)} contentClassName={contentLayoutClassName}>
        <TextShimmer className={clsx('flex-none text-[var(--dsh-content-font-size-secondary,13px)] leading-[calc(24px+var(--dsh-content-font-delta,0px))] [color:inherit]', titleClassName)}>{title}</TextShimmer>
        {(keepContentWhenOpen || !open) && collapsedContent}
        </TextShimmer>
      </div>
      </div>
      <div id={contentId} hidden={!open}>{open && children}</div>
    </div>
  )
}
