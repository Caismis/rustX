import { useState, useSyncExternalStore } from 'react';
import { localeController } from './controller';
import { translator, displayText, type DisplayText } from './translation';
export function useLocale() {
  return useSyncExternalStore(localeController.subscribe, localeController.getSnapshot, localeController.getSnapshot);
}
/** The function closes over a subscribed immutable locale, never a mutable
 * global read. Pure presentation helpers receive this same typed function. */
export function useTranslation() { return translator(useLocale().active); }

/** Retain the meaning of browser notices, not text in the old locale. */
export function useNotice(initial: DisplayText = '') {
  const [value, setValue] = useState<DisplayText>(initial);
  const tx = useTranslation();
  return [displayText(tx, value), setValue] as const;
}
