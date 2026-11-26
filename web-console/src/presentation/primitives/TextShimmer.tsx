/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
/** Text activity animation shared by a row and its nested text fragments. */
import { createContext, memo, useContext, type ReactNode } from 'react'
import clsx from 'clsx'

const DecorativeCopy = createContext<boolean | undefined>(undefined)

/** Text and activity supplied by the owning row. */
export interface TextShimmerProps {
  /** Text or presentational children with text in nested TextShimmer instances. No effects or element ids. */
  children: ReactNode
  /** Whether to animate; nested instances share the containing row's activity. */
  active?: boolean | undefined
  className?: string | undefined
  /** Layout class applied equally to the base and decorative content. */
  contentClassName?: string | undefined
}

function TextContent({ children, className }: Pick<TextShimmerProps, 'children' | 'className'>) {
  const decorative = useContext(DecorativeCopy)
  const generated = decorative === true && typeof children === 'string'
  return (
    <span className={clsx('[color:inherit] data-[shimmer-text]:after:content-[attr(data-shimmer-text)]', className)} data-shimmer-text={generated ? children : undefined}>
      {generated ? null : children}
    </span>
  )
}

/**
 * Render text with one shared highlight while retaining selectable, accessible content.
 * Nested instances inherit the outer animation. Keep icons outside; mark decorative
 * separators with data-shimmer-decoration so their background follows the highlight.
 * Active children also render in an inert, clipped decoration; supply only presentation.
 * @param props - localized text, running state, and owner styling.
 * @returns retained text and its optional decorative highlight.
 */
export const TextShimmer = memo(function TextShimmer({ children, active = false, className, contentClassName }: TextShimmerProps) {
  const decorative = useContext(DecorativeCopy)
  if (decorative !== undefined) return <TextContent className={className}>{children}</TextContent>
  const content = typeof children === 'string' ? <TextContent>{children}</TextContent> : children
  return (
    <span className={clsx('relative inline-grid w-max max-w-full min-w-0 align-top [color:inherit]', className)} data-shimmer={active || undefined}>
      <DecorativeCopy.Provider value={false}>
        <span className={clsx('flex items-center min-w-0', contentClassName)}>{content}</span>
      </DecorativeCopy.Provider>
      {active && (
        <span className="absolute inset-0 overflow-clip pointer-events-none select-none motion-reduce:hidden" aria-hidden="true" inert>
          <span className="absolute inset-0 overflow-hidden [color:var(--dsw-alias-label-shimmer)] [mask-image:linear-gradient(105deg,transparent_0%,black_40%_60%,transparent_100%)] [transform:translateX(-100%)] animate-row-shimmer-sweep motion-reduce:animate-none [&_*]:[color:inherit]! [&_[data-shimmer-decoration]]:bg-current">
            <DecorativeCopy.Provider value>
              <span className={clsx('flex items-center min-w-0 w-full h-full [transform:translateX(100%)] animate-row-shimmer-highlight motion-reduce:animate-none', contentClassName)}>{content}</span>
            </DecorativeCopy.Provider>
          </span>
        </span>
      )}
    </span>
  )
})
