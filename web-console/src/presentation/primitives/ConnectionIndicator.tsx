/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ConnectionIndicator; see PROVENANCE.md. */
import { useEffect, useState } from 'react'
import { IconCheckOutline16, IconRefreshOutline16 } from './icons'
import { StateDot } from './StateDot'

/** Visual state rendered by {@link ConnectionIndicator}. */
export type ConnectionIndicatorState =
  | 'disconnected'
  | 'connecting'
  | 'recovered'

/** Exit-transition length; keep equal to the `.leaving` transition duration in the stylesheet. */
const EXIT_MS = 150

/**
 * Render an inline connection-recovery control. The outage and retry-attempt
 * states are one button whose static label already names the retry action;
 * clicking it requests an immediate reconnect. The indicator animates in on
 * appearance and fades out for {@link EXIT_MS} before unmounting.
 * @param props.state - visible outage, retry-attempt, or recovered state.
 * @param props.disconnectedLabel - localized outage text naming the retry action.
 * @param props.connectingLabel - localized retry text followed by the attempt dots.
 * @param props.recoveredLabel - localized recovery confirmation.
 * @param props.reconnectActionLabel - accessible label for the outage action.
 * @param props.restartActionLabel - accessible label for replacing an active attempt.
 * @param props.onReconnect - request an immediate reconnect attempt.
 * @returns the indicator, or null when no connection feedback is active.
 */
export function ConnectionIndicator({
  state,
  disconnectedLabel,
  connectingLabel,
  recoveredLabel,
  reconnectActionLabel,
  restartActionLabel,
  onReconnect,
  compact = false,
}: {
  state: ConnectionIndicatorState | undefined
  compact?: boolean
  disconnectedLabel: string
  connectingLabel: string
  recoveredLabel: string
  reconnectActionLabel: string
  restartActionLabel: string
  onReconnect: () => void
}) {
  const [rendered, setRendered] = useState(state)
  const leaving = state === undefined && rendered !== undefined
  useEffect(() => {
    if (state !== undefined) {
      setRendered(state)
      return
    }
    if (rendered === undefined) return
    const timeout = window.setTimeout(() => { setRendered(undefined) }, EXIT_MS)
    return () => { window.clearTimeout(timeout) }
  }, [state, rendered])

  if (rendered === undefined) return null
  const leavingClass = `${leaving ? ' opacity-0' : ''}${compact ? ' grid-cols-[14px]! w-[28px] justify-center p-0! [&_.label]:absolute [&_.label]:w-[1px] [&_.label]:h-[1px] [&_.label]:p-0 [&_.label]:-m-[1px] [&_.label]:overflow-hidden [&_.label]:[clip-path:inset(50%)] [&_.label]:whitespace-nowrap' : ''}`
  if (rendered === 'recovered') {
    return (
      <div
        className={`flex-none inline-grid grid-cols-[14px_max-content] items-center gap-x-[4px] h-[28px] px-[8px] py-0 box-border [border:1px_solid_transparent] rounded-[var(--dsw-radius-sm)] [font-family:inherit] text-[12px] font-medium leading-[18px] whitespace-nowrap transition-[background-color,color,border-color,opacity] duration-150 animate-indicator-enter motion-reduce:animate-none motion-reduce:transition-none bg-[var(--dsw-alias-state-success-tertiary)] [color:var(--dsw-alias-state-success-primary)] [border-color:color-mix(in_srgb,var(--dsw-alias-state-success-primary)_20%,transparent)]!${leavingClass}`}
        role="status"
        aria-label={recoveredLabel}
      >
        <span className="grid place-items-center w-[14px] h-[14px] [&>svg]:[color:inherit]" aria-hidden="true"><IconCheckOutline16 size={14} /></span>
        <span className="label text-left">{recoveredLabel}</span>
      </div>
    )
  }

  const connecting = rendered === 'connecting'
  return (
    <button
      type="button"
      className={`flex-none inline-grid grid-cols-[14px_max-content] items-center gap-x-[4px] h-[28px] px-[8px] py-0 box-border [border:1px_solid_transparent] rounded-[var(--dsw-radius-sm)] [font-family:inherit] text-[12px] font-medium leading-[18px] whitespace-nowrap transition-[background-color,color,border-color,opacity] duration-150 animate-indicator-enter motion-reduce:animate-none motion-reduce:transition-none bg-[var(--dsw-alias-state-warn-tertiary)] [color:var(--dsw-alias-state-warn-label)] [border-color:color-mix(in_srgb,var(--dsw-alias-state-warn-label)_20%,transparent)]! cursor-pointer hover:bg-[color-mix(in_srgb,var(--dsw-alias-state-warn-tertiary),var(--dsw-alias-state-warn-primary)_6%)] active:bg-[color-mix(in_srgb,var(--dsw-alias-state-warn-tertiary),var(--dsw-alias-state-warn-primary)_10%)] focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:outline-offset-2${leavingClass}`}
      data-phase={rendered}
      aria-label={connecting ? restartActionLabel : reconnectActionLabel}
      onClick={onReconnect}
    >
      <span className="grid place-items-center w-[14px] h-[14px] [&>svg]:[color:inherit]" aria-hidden="true">
        {connecting
          ? <StateDot state="ongoing" />
          : <IconRefreshOutline16 size={14} />}
      </span>
      <span className="label text-left">
        {connecting
          ? (
            <>
              {connectingLabel}
              <span className="inline-block w-[1em] text-left" aria-hidden="true">
                <span>.</span>
                <span className="animate-reveal-second-dot motion-reduce:animate-none">.</span>
                <span className="animate-reveal-third-dot motion-reduce:animate-none">.</span>
              </span>
            </>
          )
          : disconnectedLabel}
      </span>
    </button>
  )
}
