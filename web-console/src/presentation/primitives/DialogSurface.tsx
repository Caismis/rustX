import { createContext, useCallback, useContext, useState, type ReactNode, type Ref } from 'react';
import { createPortal } from 'react-dom';
import { Dialog } from '@base-ui/react/dialog';
import { AlertDialog } from '@base-ui/react/alert-dialog';

const DialogPortalContext = createContext<HTMLElement | null>(null);
const OverlayActiveContext = createContext(true);
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
  const attach = useCallback((node: HTMLElement | null) => {
    if (typeof contentRef === 'function') contentRef(node);
    else if (contentRef) contentRef.current = node;
  }, [contentRef]);
  const Root = role === 'alertdialog' ? AlertDialog.Root : Dialog.Root;
  return <Root open={open && active} onOpenChange={(next, details) => {
    if (details.reason === 'escape-key' && (details.event.target as Element)?.closest?.('[role="menu"], [role="listbox"]')) {
      details.cancel();
      return;
    }
    if (!next && active) onClose();
  }}>
    <Dialog.Portal ref={setPortal} container={container} keepMounted={open}>
      <Dialog.Viewport className={overlayClassName}>
        <div className={panelClassName} style={panelClassName ? undefined : { display: 'contents' }}>
          <Dialog.Popup render={<section />} ref={attach} className={className} aria-label={title}
            initialFocus={initialFocus} finalFocus={active ? finalFocus : false}>
            <DialogPortalContext value={portal}>{children}</DialogPortalContext>
          </Dialog.Popup>
        </div>
      </Dialog.Viewport>
    </Dialog.Portal>
  </Root>;
}
