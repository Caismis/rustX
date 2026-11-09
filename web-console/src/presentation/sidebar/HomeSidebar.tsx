/* Copyright (c) 2026 DeepSeek. MIT. Adapted Home sidebar; see PROVENANCE.md. */
import { useEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../locale/react';
import { IconNewChatOutline16, IconPanelLeftOutline16 } from '../primitives/icons';
import { Tooltip } from '../primitives/Tooltip';
import css from './HomeSidebar.module.css';

/** Home owns conversation browsing. Primary destinations live in a separate rail. */
export function HomeSidebar({ width, toggleSidebar, startSession, browser, footer }: {
  width: number; toggleSidebar: () => void; startSession: () => void;
  browser: ReactNode; footer?: ReactNode;
}) {
  const tx = useTranslation();
  const [pointerInside, setPointerInside] = useState(false);
  const linger = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const cancelLinger = () => { clearTimeout(linger.current); linger.current = undefined; };
  useEffect(() => cancelLinger, []);
  return <div aria-label={tx('sidebar:sidebar-root.sidebar')} className={clsx(css.root, !pointerInside && css.quietBars)} style={{ width }}
    onPointerEnter={() => { cancelLinger(); setPointerInside(true); }}
    onPointerLeave={() => { cancelLinger(); linger.current = setTimeout(() => setPointerInside(false), 2000); }}>
    <div className={css.logoRow} data-window-drag>
      <span className={css.brand}><span className={css.brandMark} aria-hidden>rX</span>rustX</span>
      <Tooltip label={tx('sidebar:toggle.collapse')} delayMs={500}>
        <button type="button" data-sidebar-toggle className={css.iconButton} aria-label={tx('sidebar:toggle.collapse')} onClick={toggleSidebar}><IconPanelLeftOutline16 /></button>
      </Tooltip>
    </div>
    <button type="button" className={css.newSession} aria-label={tx('sidebar:session.new.label')} onClick={startSession}>
      <IconNewChatOutline16 size={14}/><span>{tx('sidebar:session.new')}</span>
    </button>
    <div className={css.regionArea}>{browser}</div>
    {footer && <div className={css.footer}>{footer}</div>}
  </div>;
}
