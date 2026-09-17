import { useEffect, useRef, type RefObject } from 'react';
// Only live, portaled presentation layers; no product or persistence state.
const layers: HTMLElement[] = [];
let rootWasInert = false;
function exposeTopLayer() {
  const root = document.getElementById('root');
  if (root) root.inert = layers.length > 0 || rootWasInert;
  for (const layer of layers) layer.parentElement!.inert = layer !== layers.at(-1);
}
/** Focus/Escape and background isolation for the upstream portaled modal DOM. */
export function useModalFocus(open: boolean, ref: RefObject<HTMLElement | null>, onClose: () => void) {
  const close = useRef(onClose);
  close.current = onClose;
  useEffect(() => {
    const dialog = ref.current;
    if (!open || !dialog) return;
    const previous = document.activeElement as HTMLElement | null;
    if (!layers.length) rootWasInert = document.getElementById('root')?.inert ?? false;
    layers.push(dialog);
    exposeTopLayer();
    const controls = () => Array.from(dialog.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), a[href], [tabindex="0"]')).filter(node => !node.hidden && !node.closest('[inert]'));
    (dialog.querySelector<HTMLElement>('[autofocus]') ?? controls()[0] ?? dialog).focus();
    const trap = (event: KeyboardEvent) => {
      if (layers.at(-1) !== dialog || event.defaultPrevented) return;
      if (event.key === 'Escape') { event.preventDefault(); close.current(); return; }
      if (event.key !== 'Tab') return;
      const nodes = controls(), first = nodes[0], last = nodes.at(-1);
      if (!first) { event.preventDefault(); dialog.focus(); }
      else if (event.shiftKey && (document.activeElement === first || !dialog.contains(document.activeElement))) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && (document.activeElement === last || !dialog.contains(document.activeElement))) { event.preventDefault(); first.focus(); }
    };
    document.addEventListener('keydown', trap);
    return () => {
      document.removeEventListener('keydown', trap);
      const wasTop = layers.at(-1) === dialog;
      layers.splice(layers.indexOf(dialog), 1);
      if (dialog.parentElement) dialog.parentElement.inert = false;
      exposeTopLayer();
      if (wasTop && previous?.isConnected) previous.focus();
    };
  }, [open, ref]);
}
