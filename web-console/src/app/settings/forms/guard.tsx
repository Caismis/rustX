import { createContext, useCallback, useContext, useEffect, useRef, useState } from 'react';
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

  const guard = useCallback((proceed: () => void) => {
    if (surfaces.current.size === 0) { proceed(); return; }
    pendingRef.current = proceed;
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

  return <GuardContext.Provider value={{ reportSurface, dirty, guard }}>
    {children}
    <Modal open={confirming} onClose={keepEditing} title={tx('settings:unsaved.title')}
      rootClassName={css.aboveDialogs}
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
