import { useCallback, useLayoutEffect, useRef, type RefObject } from 'react';
import { useSearchableHidden, type HiddenTransition } from './searchable-hidden';

const FADE_OUT_MS = 70;
const CLOSE_DELAY_MS = 50;
const CLOSE_MS = 110;
const SETTLE_SLACK_MS = 80;
const MOTION_SCOPE = '[data-chat-motion]';
const EASING = 'cubic-bezier(0.4, 0, 0.2, 1)';

function motionEnabled(element: HTMLElement): boolean {
  if (element.closest(MOTION_SCOPE) === null) return false;
  return typeof matchMedia !== 'function' || !matchMedia('(prefers-reduced-motion: reduce)').matches;
}

function clearElementMotion(element: HTMLElement) {
  const { style } = element;
  style.height = '';
  style.marginTop = '';
  style.opacity = '';
  style.overflow = '';
  style.transition = '';
  delete element.dataset.chatMotion;
}

/**
 * Apply `hidden="until-found"` with smooth fade/height fold transitions inside
 * `[data-chat-motion]`. Respects `prefers-reduced-motion: reduce` and commits
 * synchronously when the element has zero measured height (e.g. offscreen/jsdom).
 */
export function useFlowHidden(hidden: boolean, reveal: () => void, grow = false): RefObject<HTMLDivElement | null> {
  const mountedElement = useRef<HTMLElement | null>(null);
  const cleanupRef = useRef<(() => void) | null>(null);

  const cancel = useCallback(() => {
    cleanupRef.current?.();
    cleanupRef.current = null;
  }, []);

  useLayoutEffect(() => () => {
    cancel();
    mountedElement.current = null;
  }, [cancel]);

  const transition = useCallback<HiddenTransition>((element, nextHidden, commit) => {
    const mounted = mountedElement.current === element;
    mountedElement.current = element;
    const animate = mounted && motionEnabled(element);

    if (nextHidden) {
      if (element.hasAttribute('hidden') || element.dataset.chatMotion === 'collapse') return;
      cancel();
      if (!animate) {
        commit();
        return;
      }
      const height = element.offsetHeight;
      const marginTop = Number.parseFloat(getComputedStyle(element).marginTop) || 0;
      if (height === 0 && marginTop === 0) {
        commit();
        return;
      }
      element.dataset.chatMotion = 'collapse';
      const { style } = element;
      style.transition = 'none';
      style.overflow = 'hidden';
      style.height = `${height}px`;
      style.marginTop = `${marginTop}px`;
      void element.offsetHeight;
      style.transition = `opacity ${FADE_OUT_MS}ms ${EASING}, height ${CLOSE_MS}ms ${EASING} ${CLOSE_DELAY_MS}ms, margin-top ${CLOSE_MS}ms ${EASING} ${CLOSE_DELAY_MS}ms`;
      style.opacity = '0';
      style.height = '0px';
      style.marginTop = '0px';

      let settled = false;
      const finish = () => {
        if (settled) return;
        settled = true;
        cleanup();
        commit();
      };
      const onEnd = (event: TransitionEvent) => {
        if (event.target === element && event.propertyName === 'height') finish();
      };
      const timer = setTimeout(finish, CLOSE_DELAY_MS + CLOSE_MS + SETTLE_SLACK_MS);
      const cleanup = () => {
        element.removeEventListener('transitionend', onEnd);
        clearTimeout(timer);
        clearElementMotion(element);
        if (cleanupRef.current === cleanup) cleanupRef.current = null;
      };
      element.addEventListener('transitionend', onEnd);
      cleanupRef.current = cleanup;
      return;
    }

    cancel();
    const wasHidden = element.hasAttribute('hidden');
    commit();
    if (!wasHidden || !animate || !grow) return;
    const height = element.offsetHeight;
    if (height === 0) return;
    const marginTop = Number.parseFloat(getComputedStyle(element).marginTop) || 0;
    element.dataset.chatMotion = 'reveal';
    const { style } = element;
    style.transition = 'none';
    style.overflow = 'hidden';
    style.opacity = '0';
    style.height = '0px';
    style.marginTop = '0px';
    void element.offsetHeight;
    style.transition = `height ${CLOSE_MS}ms ${EASING}, margin-top ${CLOSE_MS}ms ${EASING}, opacity ${CLOSE_MS}ms ${EASING}`;
    style.height = `${height}px`;
    style.marginTop = `${marginTop}px`;
    style.opacity = '1';

    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      cleanup();
    };
    const onEnd = (event: TransitionEvent) => {
      if (event.target === element && event.propertyName === 'height') finish();
    };
    const timer = setTimeout(finish, CLOSE_MS + SETTLE_SLACK_MS);
    const cleanup = () => {
      element.removeEventListener('transitionend', onEnd);
      clearTimeout(timer);
      clearElementMotion(element);
      if (cleanupRef.current === cleanup) cleanupRef.current = null;
    };
    element.addEventListener('transitionend', onEnd);
    cleanupRef.current = cleanup;
  }, [cancel, grow]);

  return useSearchableHidden(hidden, reveal, transition);
}
