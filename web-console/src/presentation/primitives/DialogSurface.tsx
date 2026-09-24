import { createContext, useCallback, useContext, useState, type ReactNode, type Ref } from 'react';
import { Dialog } from '@base-ui/react/dialog';
import { AlertDialog } from '@base-ui/react/alert-dialog';

const DialogPortalContext = createContext<HTMLElement | null>(null);
/** Controls share the modal portal, while retaining their own focus scope. */
export const useDialogPortal = () => useContext(DialogPortalContext) ?? document.body;

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
  const [portal, setPortal] = useState<HTMLDivElement | null>(null);
  const attach = useCallback((node: HTMLElement | null) => {
    if (typeof contentRef === 'function') contentRef(node);
    else if (contentRef) contentRef.current = node;
  }, [contentRef]);
  const Root = role === 'alertdialog' ? AlertDialog.Root : Dialog.Root;
  return <Root open={open} onOpenChange={(next, details) => {
    if (details.reason === 'escape-key' && (details.event.target as Element)?.closest?.('[role="menu"], [role="listbox"]')) {
      details.cancel();
      return;
    }
    if (!next) onClose();
  }}>
    <Dialog.Portal ref={setPortal} container={document.body}>
      <Dialog.Viewport className={overlayClassName}>
        <div className={panelClassName} style={panelClassName ? undefined : { display: 'contents' }}>
          <Dialog.Popup render={<section />} ref={attach} className={className} aria-label={title}
            initialFocus={initialFocus} finalFocus={finalFocus}>
            <DialogPortalContext value={portal}>{children}</DialogPortalContext>
          </Dialog.Popup>
        </div>
      </Dialog.Viewport>
    </Dialog.Portal>
  </Root>;
}
