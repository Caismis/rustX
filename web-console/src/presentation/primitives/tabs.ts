import type { KeyboardEvent } from 'react';

/** Horizontal automatic-activation tabs. Only presentation buttons are invoked;
 * callers retain ownership of selection. Modified shortcuts and vertical scroll
 * keys retain their browser behavior. */
export function navigateTabs(event: KeyboardEvent<HTMLElement>) {
  if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || !['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
  const tabs = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="tab"]'))
    .filter(tab => !tab.disabled && tab.getClientRects().length > 0);
  const index = tabs.indexOf(event.target as HTMLButtonElement);
  if (index < 0) return;
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1
    : (index + (event.key === 'ArrowRight' ? 1 : tabs.length - 1)) % tabs.length;
  event.preventDefault();
  tabs[next].focus();
  tabs[next].click();
}
