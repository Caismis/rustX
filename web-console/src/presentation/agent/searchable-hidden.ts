import { useEffect, useLayoutEffect, useRef, type RefObject } from 'react';

export type HiddenTransition = (element: HTMLElement, hidden: boolean, commit: () => void) => void;

/**
 * Apply `hidden="until-found"` without unmounting a stable subtree, so browser
 * find-in-page (`beforematch`) can reveal folded process rows automatically.
 */
export function useSearchableHidden(
  hidden: boolean,
  reveal: () => void,
  transition?: HiddenTransition,
): RefObject<HTMLDivElement | null> {
  const ref = useRef<HTMLDivElement | null>(null);
  const revealRef = useRef(reveal);
  revealRef.current = reveal;

  useLayoutEffect(() => {
    const element = ref.current;
    if (element === null) return;
    if (hidden && element.contains(element.ownerDocument.activeElement)) {
      revealRef.current();
      return;
    }
    const commit = (): void => {
      if (hidden) element.setAttribute('hidden', 'until-found');
      else element.removeAttribute('hidden');
    };
    if (transition === undefined) commit();
    else transition(element, hidden, commit);
  }, [hidden, transition]);

  useEffect(() => {
    const element = ref.current;
    if (element === null) return;
    const onBeforeMatch = () => { revealRef.current(); };
    element.addEventListener('beforematch', onBeforeMatch);
    return () => { element.removeEventListener('beforematch', onBeforeMatch); };
  }, []);

  return ref;
}
