/* Copyright (c) 2026 DeepSeek. MIT. Adapted Home sidebar; see PROVENANCE.md. */
import { useEffect, useRef, useState, type ReactNode } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../locale/react';
import { IconNewChatOutline16, IconPanelLeftOutline16 } from '../primitives/icons';
import { Tooltip } from '../primitives/Tooltip';

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
  return <div aria-label={tx('sidebar:sidebar-root.sidebar')} className={clsx('[--dsh-sidebar-inline-padding:12px] [--dsh-scrollbar-thumb:var(--dsw-alias-scrollbar-bg-l2)] [--dsh-scrollbar-thumb-hover:var(--dsw-alias-scrollbar-hover-l2)] flex flex-col h-full px-[var(--dsh-sidebar-inline-padding)] py-[6px] box-border bg-[var(--dsw-specific-sidebar-fill)] [color:var(--dsw-alias-label-primary)] text-[14px] [[data-platform=darwin]_&]:bg-transparent', !pointerInside && '[--dsh-scrollbar-thumb:transparent] [--dsh-scrollbar-thumb-hover:transparent]')} style={{ width }}
    onPointerEnter={() => { cancelLinger(); setPointerInside(true); }}
    onPointerLeave={() => { cancelLinger(); linger.current = setTimeout(() => setPointerInside(false), 2000); }}>
    <div className="flex-none flex items-center gap-[8px] h-[60px] pt-[8px] pr-0 pb-[8px] pl-[4px] mb-[4px] box-border [[data-platform=darwin]:not([data-fullscreen])_&]:min-h-[60px] [[data-windows-titlebar]_&]:h-[40px] [[data-windows-titlebar]_&]:m-0 [[data-windows-titlebar]_&]:p-0 [[data-windows-titlebar]_&]:pl-[4px]" data-window-drag>
      <img className="block h-full w-auto max-w-full min-w-0 mr-auto object-contain [[data-ds-dark-theme]_&]:content-[url(/rustx-logo-dark.png)]" src="/rustx-logo.png" width={414} height={240} alt="rustX" />
      <Tooltip label={tx('sidebar:toggle.collapse')} delayMs={500}>
        <button type="button" data-sidebar-toggle className="flex-none inline-flex items-center justify-center w-[28px] h-[28px] p-0 border-0 rounded-[var(--dsw-radius-sm)] bg-transparent cursor-pointer [color:var(--dsw-alias-label-secondary)] hover:bg-[var(--dsw-alias-interactive-bg-hover)] focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:outline-offset-[-2px]" aria-label={tx('sidebar:toggle.collapse')} onClick={toggleSidebar}><IconPanelLeftOutline16 /></button>
      </Tooltip>
    </div>
    <button type="button" className="flex-none flex items-center justify-center gap-[6px] h-[38px] px-[12px] py-[8px] mt-0 mx-[2px] mb-[12px] [border:0.5px_solid_var(--dsw-alias-border-l3)] rounded-[var(--dsw-radius-md)] bg-[var(--dsw-alias-button-elevated-fill)] [color:var(--dsw-alias-label-primary)] [font:inherit] font-medium leading-[22px] cursor-pointer hover:bg-[var(--dsw-alias-button-floating-hover)] focus-visible:[outline:var(--dsw-focus-ring-width)_solid_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] focus-visible:outline-offset-[-2px]" aria-label={tx('sidebar:session.new.label')} onClick={startSession}>
      <IconNewChatOutline16 size={14}/><span>{tx('sidebar:session.new')}</span>
    </button>
    <div className="flex-1 min-h-0 flex flex-col ml-[-4px] mr-[calc(-1*var(--dsh-sidebar-inline-padding))] pl-[4px] overflow-hidden">{browser}</div>
    {footer && <div className="flex-none flex items-center gap-[8px]">{footer}</div>}
  </div>;
}
