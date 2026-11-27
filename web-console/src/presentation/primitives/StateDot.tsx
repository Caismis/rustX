/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import clsx from 'clsx'
import { Spinner } from '@/components/ui/spinner'

/**
 * State semantic: green done / amber user-attention / blue running ring /
 * red error / grey idle for a tracked subject with nothing in progress.
 */
export type StateDotState = 'done' | 'warning' | 'ongoing' | 'error' | 'idle'

/**
 * Render a state dot.
 * @param props.state - which of `done`, `warning`, `ongoing`, `error`, or `idle` to show.
 * @param props.size - outer diameter in px (default 10, the figma size).
 * @param props.className - extra class for layout placement.
 * @returns the dot element (aria-hidden; pair with text for accessibility).
 */
export function StateDot({ state, size = 10, className }: {
  state: StateDotState
  size?: number | undefined
  className?: string | undefined
}) {
  if (state === 'ongoing') {
    return <Spinner className={className} width={size} height={size} role={undefined} aria-label={undefined} aria-hidden="true" data-state="ongoing" />
  }
  // Positioning comes from the consumer when it supplies one (e.g. TerminalBlock's
  // absolute runState): `relative` would win the cascade over `absolute` in the
  // tailwind stylesheet order, silently pulling the dot back into flow.
  const positioned = className !== undefined && /\b(absolute|fixed|sticky|relative)\b/.test(className)
  return (
    <span
      className={clsx(!positioned && 'relative', 'inline-block flex-none data-[state=done]:[color:var(--dsw-alias-state-success-primary)] data-[state=warning]:[color:var(--dsw-alias-state-warn-primary)] data-[state=error]:[color:var(--dsw-alias-state-error-primary)] data-[state=idle]:[color:var(--dsw-alias-label-tertiary)] before:content-[""] before:absolute before:inset-0 before:rounded-[50%] before:[corner-shape:round]! before:bg-current before:opacity-10 after:content-[""] after:absolute after:inset-[20%] after:rounded-[50%] after:[corner-shape:round]! after:bg-current', className)}
      data-state={state}
      style={{ width: size, height: size }}
      aria-hidden="true"
    />
  )
}
