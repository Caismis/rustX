import { createContext, useCallback, useContext, useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { useDialogPortal } from '../../presentation/primitives/DialogSurface';
import { IconWarningOutline16 } from '../../presentation/primitives/icons';
import { Toast } from '../../presentation/primitives/Toast';

type Notice = { id: string; text: string; portal: HTMLElement; tone?: 'success'; sequence: number; expires: number };
type Publish = (id: string, text: string, portal: HTMLElement, tone?: 'success') => void;
const Feedback = createContext<Publish | undefined>(undefined);

/** One transient feedback surface per Settings owner, including its dialogs. */
export function SettingsFeedback({ active = true, children }: { active?: boolean; children: ReactNode }) {
  const [notices, setNotices] = useState<Notice[]>([]);
  const sequence = useRef(0);
  const publish = useCallback<Publish>((id, text, portal, tone) => {
    const now = Date.now();
    const entry = text && active ? { id, text, portal, tone, sequence: ++sequence.current, expires: now + 6000 } : undefined;
    setNotices(current => {
      const remaining = current.filter(item => item.id !== id && item.expires > now);
      return entry ? [...remaining.slice(-3), entry] : remaining.length === current.length ? current : remaining;
    });
  }, [active]);
  const notice = notices.at(-1);
  return <Feedback value={publish}>
    {children}
    {active && notice && <Toast key={notice.sequence} text={notice.text} tone={notice.tone} icon={<IconWarningOutline16/>}
      portal={notice.portal} foreground holdMs={Math.max(0, notice.expires - Date.now() - 1000)}
      onDone={() => setNotices(current => current.filter(item => item !== notice && item.expires > Date.now()))}/>}
  </Feedback>;
}

export function useSettingsFeedback() {
  return useContext(Feedback) !== undefined;
}

/** Repeated renders of the same failure do not restart its dismissal timer.
 * Shared editors can still present their own feedback outside Settings. */
export function SettingsNotice({ text, tone, fallback, occurrence }: {
  text?: string; tone?: 'success'; fallback?: ReactNode; occurrence?: number;
}) {
  const id = useId();
  const portal = useDialogPortal();
  const publish = useContext(Feedback);
  useEffect(() => {
    if (!publish || !text) return;
    publish(id, text, portal, tone);
    return () => publish(id, '', portal);
  }, [publish, id, text, portal, tone, occurrence]);
  return !publish && text ? fallback ?? <p role={tone === 'success' ? 'status' : 'alert'}>{text}</p> : null;
}
