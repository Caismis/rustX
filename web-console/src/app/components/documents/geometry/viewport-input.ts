import { MOUSE, TOUCH } from 'three';
import type { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import type { StandardView } from './viewport';

const axisKeys = {
  Numpad1: ['front', 'back'], Numpad3: ['right', 'left'], Numpad7: ['top', 'bottom'],
} as const satisfies Record<string, readonly [StandardView, StandardView]>;
const orbitKeys = { Numpad2: 'down', Numpad4: 'left', Numpad6: 'right', Numpad8: 'up' } as const;
type OrbitDirection = typeof orbitKeys[keyof typeof orbitKeys];
type Shortcut = { kind: 'view'; view: StandardView } | { kind: 'orbit'; direction: OrbitDirection } |
  { kind: 'fit' | 'focus' | 'projection' | 'isolate' };

export function viewportShortcut(event: Pick<KeyboardEvent, 'code' | 'altKey' | 'shiftKey' | 'metaKey' | 'ctrlKey' | 'isComposing'>): Shortcut | undefined {
  if (event.isComposing || event.altKey || event.shiftKey || event.metaKey) return;
  const key = event.code;
  if (Object.hasOwn(axisKeys, key)) return { kind: 'view', view: axisKeys[key as keyof typeof axisKeys][event.ctrlKey ? 1 : 0] };
  if (event.ctrlKey) return;
  if (Object.hasOwn(orbitKeys, key)) return { kind: 'orbit', direction: orbitKeys[key as keyof typeof orbitKeys] };
  switch (key) {
    case 'Home': return { kind: 'fit' };
    case 'NumpadDecimal': return { kind: 'focus' };
    case 'Numpad5': return { kind: 'projection' };
    case 'NumpadDivide': return { kind: 'isolate' };
  }
}

/** OrbitControls owns the gesture. Track click eligibility and cancel its
 * document listeners if the browser retires capture before pointerup. */
export function bindViewportInput(canvas: HTMLCanvasElement, gizmo: SVGSVGElement, options: {
  visible(): boolean;
  controls(): OrbitControls;
  turntable(): void;
  select(event: PointerEvent): void;
  guardZoom?(x: number, y: number): void;
  clearZoomGuard?(): void;
  shortcut(action: Shortcut): void;
  cancelNavigation(): void;
}) {
  const document = canvas.ownerDocument;
  const pointers = new Set<number>();
  let click: { id: number; x: number; y: number } | undefined;
  let dolly: { id: number; x: number; y: number; previousY: number } | undefined;
  const exceedsThreshold = (event: PointerEvent) => !!click && (event.clientX - click.x) ** 2 + (event.clientY - click.y) ** 2 > 16;
  const down = (event: PointerEvent) => {
    if (!options.visible()) return;
    if (![0, 1, 2].includes(event.button)) return;
    options.clearZoomGuard?.();
    const controls = options.controls();
    if (event.button === 1) {
      event.preventDefault();
      if (event.altKey || event.metaKey) { event.stopImmediatePropagation(); return; }
    }
    // Three maps modified ROTATE to PAN. Apply the mapping after turntable
    // restoration, since a pole view can replace the controls during capture.
    const middle = event.button === 1 ? event.ctrlKey ? MOUSE.DOLLY : MOUSE.ROTATE : undefined;
    const sole = pointers.size === 0;
    pointers.add(event.pointerId);
    if (sole && event.button === 1 && event.ctrlKey && event.pointerType !== 'touch')
      dolly = { id: event.pointerId, x: event.clientX, y: event.clientY, previousY: event.clientY };
    click = sole && event.isPrimary && event.button === 0 ? { id: event.pointerId, x: event.clientX, y: event.clientY } : undefined;
    const action = middle ?? (event.button === 0 ? controls.mouseButtons.LEFT : controls.mouseButtons.RIGHT);
    const modified = event.ctrlKey || event.shiftKey || event.metaKey;
    const rotates = event.pointerType === 'touch' ? controls.touches.ONE === TOUCH.ROTATE
      : action === MOUSE.ROTATE && !modified || action === MOUSE.PAN && modified;
    if (sole && rotates) options.turntable();
    if (middle !== undefined) options.controls().mouseButtons.MIDDLE = middle;
  };
  const move = (event: PointerEvent) => {
    if (event.pointerId === dolly?.id && options.visible()) {
      options.clearZoomGuard?.();
      if (event.clientY < dolly.previousY) options.guardZoom?.(dolly.x, dolly.y);
      dolly.previousY = event.clientY;
    }
    if (click?.id === event.pointerId && exceedsThreshold(event)) click = undefined;
  };
  const up = (event: PointerEvent) => {
    if (event.pointerId === dolly?.id) { options.clearZoomGuard?.(); dolly = undefined; }
    if (!pointers.delete(event.pointerId) || click?.id !== event.pointerId) return;
    const select = options.visible() && event.button === 0 && event.target === canvas && !exceedsThreshold(event);
    click = undefined;
    if (!select) return;
    const rect = canvas.getBoundingClientRect();
    if (event.clientX >= rect.left && event.clientX < rect.right && event.clientY >= rect.top && event.clientY < rect.bottom) options.select(event);
  };
  const cancel = () => {
    options.clearZoomGuard?.();
    click = dolly = undefined;
    if (!pointers.size) return;
    const captured = [...pointers]; pointers.clear();
    options.cancelNavigation();
    for (const id of captured) if (canvas.hasPointerCapture(id)) canvas.releasePointerCapture(id);
  };
  const canceled = (event: PointerEvent) => { if (pointers.has(event.pointerId)) cancel(); };
  const aux = (event: MouseEvent) => { if (event.button === 1) { event.preventDefault(); event.stopPropagation(); } };
  const wheel = (event: WheelEvent) => {
    options.clearZoomGuard?.();
    if (options.visible() && !pointers.size && event.deltaY < 0) options.guardZoom?.(event.clientX, event.clientY);
  };
  const keydown = (event: KeyboardEvent) => {
    const focused = document.activeElement;
    if (!options.visible() || event.defaultPrevented || event.target !== focused
      || focused !== canvas && !gizmo.contains(focused)) return;
    const action = viewportShortcut(event);
    if (!action) return;
    event.preventDefault(); event.stopImmediatePropagation();
    cancel(); options.shortcut(action);
  };
  canvas.addEventListener('pointerdown', down, true);
  document.addEventListener('pointermove', move, true);
  document.addEventListener('pointerup', up, true);
  canvas.addEventListener('pointercancel', canceled, true);
  canvas.addEventListener('lostpointercapture', canceled);
  canvas.addEventListener('auxclick', aux);
  canvas.addEventListener('wheel', wheel, { capture: true, passive: true });
  canvas.addEventListener('keydown', keydown, true);
  gizmo.addEventListener('keydown', keydown, true);
  return {
    cancel,
    dispose() {
      options.clearZoomGuard?.();
      canvas.removeEventListener('pointerdown', down, true);
      document.removeEventListener('pointermove', move, true);
      document.removeEventListener('pointerup', up, true);
      canvas.removeEventListener('pointercancel', canceled, true);
      canvas.removeEventListener('lostpointercapture', canceled);
      canvas.removeEventListener('auxclick', aux);
      canvas.removeEventListener('wheel', wheel, true);
      canvas.removeEventListener('keydown', keydown, true);
      gizmo.removeEventListener('keydown', keydown, true);
      for (const id of pointers) if (canvas.hasPointerCapture(id)) canvas.releasePointerCapture(id);
      pointers.clear(); click = undefined;
    },
  };
}
