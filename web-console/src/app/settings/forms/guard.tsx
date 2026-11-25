import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { useTranslation } from '../../../locale/react';
import { Modal } from '../../../presentation/primitives/Modal';
import { Button } from '../../../presentation/primitives/Button';
import css from './guard.module.css';

/** One navigation guard shared by every surface that can hold unsaved
 * drafts (Settings pages, the resource surface, detail editors). Each mounted
 * `PageTransactionProvider` reports its dirty state and cancel handler under
 * its own token; `guard(proceed)` either runs the navigation immediately or
 * asks first. */
interface GuardContextValue {
  /** Called by a mounted transaction provider whenever its dirty state or
   * cancel handler changes. `dirty: false` clears the token. */
  reportSurface(token: object, dirty: boolean, discard?: () => void): void;
  /** Whether any mounted surface currently reports unsaved work. */
  dirty: boolean;
  /** Run `proceed` now, or show the unified discard dialog first. */
  guard(proceed: () => void): void;
}

const GuardContext = createContext<GuardContextValue | null>(null);

export function NavigationGuardProvider({ children }: { children: ReactNode }) {
  const tx = useTranslation();
  const surfaces = useRef(new Map<object, (() => void) | undefined>());
  const [dirty, setDirty] = useState(false);
  const pendingRef = useRef<(() => void) | undefined>(undefined);
  const [confirming, setConfirming] = useState(false);

  const reportSurface = useCallback((token: object, next: boolean, discard?: () => void) => {
    if (next) surfaces.current.set(token, discard); else surfaces.current.delete(token);
    setDirty(surfaces.current.size > 0);
  }, []);

  const requesterRef = useRef<HTMLElement | null>(null);

  const guard = useCallback((proceed: () => void) => {
    if (surfaces.current.size === 0) { proceed(); return; }
    pendingRef.current = proceed;
    requesterRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setConfirming(true);
  }, []);

  const abandon = useCallback(() => {
    setConfirming(false);
    for (const discard of surfaces.current.values()) discard?.();
    const proceed = pendingRef.current;
    pendingRef.current = undefined;
    proceed?.();
  }, []);

  const keepEditing = useCallback(() => {
    setConfirming(false);
    pendingRef.current = undefined;
  }, []);

  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => { event.preventDefault(); };
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);

  const value = useMemo(() => ({ reportSurface, dirty, guard }), [reportSurface, dirty, guard]);
  return <GuardContext.Provider value={value}>
    {children}
    <Modal open={confirming} onClose={keepEditing} title={tx('settings:unsaved.title')}
      rootClassName={css.aboveDialogs}
      // Keep editing must not hand the keyboard back to the rail tab that
      // asked for the navigation: focus there reads as another request and
      // the prompt reopens. The still-selected tab is active already, so it
      // accepts focus without re-requesting anything. Any other requester —
      // a resource editor's Back button, the compact section trigger — keeps
      // its place instead, and hidden or unmounted requesters fall through to
      // the visible navigation control of the active surface.
      finalFocus={() => {
        const visible = (element: HTMLElement | null | undefined): element is HTMLElement =>
          element?.isConnected === true && element.checkVisibility();
        const requester = requesterRef.current;
        requesterRef.current = null;
        const RAIL = '[data-settings-navigation] [role="tab"]';
        if (requester !== null && !requester.matches(RAIL) && visible(requester)) {
          requester.focus({ preventScroll: true });
          return false;
        }
        const selected = document.querySelector<HTMLElement>(`${RAIL}[aria-selected="true"]`);
        if (visible(selected)) {
          selected.focus({ preventScroll: true });
          return false;
        }
        const trigger = document.querySelector<HTMLElement>('[data-settings-section-trigger]');
        if (visible(trigger)) {
          trigger.focus({ preventScroll: true });
          return false;
        }
        return false;
      }}
      description={tx('settings:unsaved.description')} closeLabel={tx('settings:aria.cancel')}
      footer={<>
        <Button variant="outline" onClick={keepEditing}>{tx('settings:unsaved.cancel')}</Button>
        <Button variant="primary" onClick={abandon}>{tx('settings:unsaved.confirm')}</Button>
      </>} />
  </GuardContext.Provider>;
}

export function useNavigationGuard(): GuardContextValue {
  const ctx = useContext(GuardContext);
  if (ctx) return ctx;
  // Outside a guarded surface navigation is always free.
  return { reportSurface: () => {}, dirty: false, guard: proceed => proceed() };
}
