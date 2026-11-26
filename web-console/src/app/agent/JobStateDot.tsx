/* Copyright (c) 2026 DeepSeek. MIT. Task loader adapted from ui-primitives StateDot; see PROVENANCE.md. */
import { StateDot, type StateDotState } from '../../presentation/primitives/StateDot';
import { backgroundJobsCss } from './backgroundJobsCss';

/** Keep every visible task loader in phase, as the Harness job list does. */
function syncSpinner(element: SVGSVGElement | null): void {
  if (!element) return;
  const pin = () => { for (const animation of element.getAnimations?.({ subtree: true }) ?? []) animation.startTime = 0; };
  pin();
  element.addEventListener('animationstart', pin);
}

export function JobStateDot({ state, size = state === 'ongoing' ? 14 : 10, className = '' }: { state: StateDotState; size?: number; className?: string }) {
  if (state !== 'ongoing') return <StateDot state={state} size={size} className={`${backgroundJobsCss.marker} ${className}`}/>;
  return <svg ref={syncSpinner} className={`${backgroundJobsCss.spinner} ${className}`} data-state="ongoing" width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
    <g className={backgroundJobsCss.spinnerMotion}>
      <circle className={backgroundJobsCss.spinnerTrack} cx="12" cy="12" r="9.5"/>
      <circle className={backgroundJobsCss.spinnerArc} cx="12" cy="12" r="9.5"/>
    </g>
  </svg>;
}
