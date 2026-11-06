import { useCallback, useLayoutEffect, useRef, useState } from 'react';
import { displayText, type DisplayText } from './translation';
import { useTranslation } from './react';

/** A display occurrence, independent of native operation/diagnostic lifetime. */
export function useToastNotice(owner?: string) {
  const tx = useTranslation(), sequence = useRef(0);
  const current = useRef(owner);
  useLayoutEffect(() => { current.current = owner; }, [owner]);
  const [notice, setNotice] = useState<{ sequence: number; text: DisplayText; owner?: string }>();
  const show = useCallback((text: DisplayText) => {
    if (current.current === owner) setNotice(text === '' ? undefined : { sequence: ++sequence.current, text, owner });
  }, [owner]);
  return [notice && notice.owner === owner ? { ...notice, text: displayText(tx, notice.text) } : undefined, show] as const;
}
