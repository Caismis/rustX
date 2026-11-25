import { createContext, useCallback, useContext, useEffect, useLayoutEffect, useRef, useState, type ReactNode, type Ref } from 'react';
import { createPortal } from 'react-dom';
import { Dialog } from '@base-ui/react/dialog';
import { AlertDialog } from '@base-ui/react/alert-dialog';

const DialogPortalContext = createContext<HTMLElement | null>(null);
const OverlayActiveContext = createContext(true);
const surfaceLayers: symbol[] = [];
function isolateRoot() {
  const root = document.getElementById('root');
  if (!root) return;
  root.inert = surfaceLayers.length > 0;
}
/** Controls share the modal portal, while retaining their own focus scope. */
export const useDialogPortal = () => useContext(DialogPortalContext) ?? document.body;
export const useOverlayActive = () => useContext(OverlayActiveContext);

/** Retained pages own their portals as well as their inline content. Hidden
 * pages keep editing state without activating a modal or restoring its focus. */
export function OverlayScope({ active, className, children }: { active: boolean; className?: string; children: ReactNode }) {
  const [portal, setPortal] = useState<HTMLDivElement | null>(null);
  return <OverlayActiveContext value={active}><DialogPortalContext value={portal}>
    <div className={className} hidden={!active} inert={!active}>{portal && children}</div>
    {createPortal(<div ref={setPortal} hidden={!active} inert={!active}/>, document.body)}
  </DialogPortalContext></OverlayActiveContext>;
}

/** Shared modal behavior, with rustX-owned geometry and workflow focus targets.
 * All close focus runs through the engine's explicit lifecycle callback. */
export function DialogSurface({ open, onClose, title, role = 'dialog', overlayClassName,
  panelClassName, className, children, initialFocus, contentRef, finalFocus,
}: {
  open: boolean; onClose: () => void; title: string; role?: 'dialog' | 'alertdialog';
  overlayClassName: string; panelClassName?: string; className?: string; children: ReactNode;
  initialFocus?: Dialog.Popup.Props['initialFocus']; contentRef?: Ref<HTMLElement>;
  finalFocus?: () => HTMLElement | false;
}) {
  const active = useOverlayActive();
  const container = useDialogPortal();
  const [portal, setPortal] = useState<HTMLDivElement | null>(null);
  const [layer] = useState(() => Symbol('dialog-surface'));
  useLayoutEffect(() => {
    if (!open || !active) return;
    surfaceLayers.push(layer);
    isolateRoot();
    return () => {
      const index = surfaceLayers.indexOf(layer);
      if (index >= 0) surfaceLayers.splice(index, 1);
      isolateRoot();
    };
  }, [open, active, layer]);
  const popupRef = useRef<HTMLElement | null>(null);
  const restoreFocusRef = useRef(finalFocus);
  restoreFocusRef.current = finalFocus;
  const attach = useCallback((node: HTMLElement | null) => {
    popupRef.current = node;
    if (typeof contentRef === 'function') contentRef(node);
    else if (contentRef) contentRef.current = node;
  }, [contentRef]);
  // Many owners close by unmounting the surface rather than by holding an
  // open=false transition. Base UI cannot run finalFocus for an unmounted
  // root, so the surface performs that same explicit restoration itself when
  // focus is disappearing with its popup.
  useEffect(() => () => {
    const target = restoreFocusRef.current?.();
    const active = document.activeElement;
    const disappearing = popupRef.current?.contains(active) === true || active === document.body || active === document.documentElement;
    if (disappearing && target && target.isConnected && !target.matches(':disabled, [aria-disabled="true"]') && !target.closest('[inert]')) {
      target.focus({ preventScroll: true });
    }
  }, []);
  const exactFinalFocus = finalFocus ? () => {
    const target = finalFocus();
    if (target !== false && target?.isConnected && !target.matches(':disabled, [aria-disabled="true"]')) {
      const ownerDocument = target.ownerDocument;
      setTimeout(() => {
        const active = ownerDocument.activeElement;
        // The close lifecycle owns focus only while it is still disappearing
        // with this popup (or has nowhere else to be). Focus that already
        // moved to another control is newer user intent and must win.
        const closeOwned = active === ownerDocument.body || active === ownerDocument.documentElement || active === target
          || popupRef.current?.contains(active) === true;
        if (closeOwned && target.isConnected && !target.matches(':disabled, [aria-disabled="true"]') && !target.closest('[inert]')) target.focus({ preventScroll: true });
      }, 0);
    }
    return false;
  } : undefined;
  const Root = role === 'alertdialog' ? AlertDialog.Root : Dialog.Root;
  return <Root open={open && active} onOpenChange={(next, details) => {
    if (details.reason === 'escape-key' && (details.event.defaultPrevented || (details.event.target as Element)?.closest?.('[role="menu"], [role="listbox"]') || surfaceLayers.at(-1) !== layer)) {
      details.cancel();
      return;
    }
    if (!next && active) {
      // Closing is the user's leaving signal for whatever held focus inside:
      // blur it while the popup is still mounted so deferred-commit fields
      // apply exactly as an explicit focus move would.
      const focused = document.activeElement;
      if (focused instanceof HTMLElement && popupRef.current?.contains(focused) === true) focused.blur();
      onClose();
    }
  }}>
    <Dialog.Portal ref={setPortal} container={container} keepMounted={open}>
      <Dialog.Viewport className={overlayClassName}>
        <div className={panelClassName} style={panelClassName ? undefined : { display: 'contents' }}>
          <Dialog.Popup render={<section />} ref={attach} className={className} aria-label={title}
            aria-hidden={!open} inert={!open}
            initialFocus={initialFocus} finalFocus={finalFocus ? exactFinalFocus : finalFocus}>
            <DialogPortalContext value={portal}>{children}</DialogPortalContext>
          </Dialog.Popup>
        </div>
      </Dialog.Viewport>
    </Dialog.Portal>
  </Root>;
}
