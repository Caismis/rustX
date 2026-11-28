import { expect, it, vi } from 'vitest';
import { MOUSE, TOUCH } from 'three';
import type { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { bindViewportInput, viewportShortcut } from '../src/app/components/documents/geometry/viewport-input';

const key = (code: string, modifiers: Partial<KeyboardEvent> = {}) => viewportShortcut({
  code, ctrlKey: false, shiftKey: false, altKey: false, metaKey: false, isComposing: false, ...modifiers,
});

it.each([
  ['Numpad1', 'front', 'back'], ['Numpad3', 'right', 'left'], ['Numpad7', 'top', 'bottom'],
])('maps %s and its opposite by physical keypad code', (code, view, opposite) => {
  expect(key(code)).toEqual({ kind: 'view', view });
  expect(key(code, { ctrlKey: true })).toEqual({ kind: 'view', view: opposite });
});
it.each(['Digit1', 'Delete', 'Period', 'Slash', 'End', 'PageDown'])('leaves ordinary %s input to its owner', code => {
  expect(key(code)).toBeUndefined();
});
it('ignores composing and extra modifiers, with Ctrl reserved for opposite views', () => {
  for (const modifier of ['shiftKey', 'altKey', 'metaKey', 'isComposing'] as const) expect(key('Numpad1', { [modifier]: true })).toBeUndefined();
  expect(key('Numpad5', { ctrlKey: true })).toBeUndefined();
});

function inputFixture() {
  const canvas = document.createElement('canvas');
  const axis = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  canvas.tabIndex = 0; document.body.append(canvas, axis);
  canvas.hasPointerCapture = vi.fn(() => true); canvas.releasePointerCapture = vi.fn();
  vi.spyOn(canvas, 'getBoundingClientRect').mockReturnValue({ left: 0, top: 0, right: 100, bottom: 100 } as DOMRect);
  const controls = { mouseButtons: { LEFT: MOUSE.PAN, MIDDLE: MOUSE.ROTATE, RIGHT: MOUSE.PAN }, touches: { ONE: TOUCH.PAN } };
  const actions = { select: vi.fn(), shortcut: vi.fn(), turntable: vi.fn(), cancelNavigation: vi.fn() };
  const connection = bindViewportInput(canvas, axis, { visible: () => true, controls: () => controls as OrbitControls, ...actions });
  const pointer = (type: string, values: PointerEventInit = {}) => {
    const event = new PointerEvent(type, { pointerId: 1, isPrimary: true, pointerType: 'mouse', clientX: 20, clientY: 20, cancelable: true, bubbles: true, ...values });
    canvas.dispatchEvent(event);
    return event;
  };
  return { canvas, pointer, controls, actions, dispose() { connection.dispose(); canvas.remove(); axis.remove(); } };
}

it('keeps middle routing temporary and retires the entire pointer after capture loss', () => {
  const f = inputFixture();
  try {
    expect(f.pointer('pointerdown', { button: 1, ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(true);
    expect(f.controls.mouseButtons.MIDDLE).toBe(MOUSE.DOLLY);
    f.pointer('lostpointercapture', { button: 1 });
    expect(f.actions.cancelNavigation).toHaveBeenCalledOnce();
    expect(f.canvas.releasePointerCapture).toHaveBeenCalledExactlyOnceWith(1);
    f.pointer('pointerup', { button: 1 });
    f.pointer('pointerdown', { button: 1, shiftKey: true });
    expect(f.controls.mouseButtons.MIDDLE).toBe(MOUSE.ROTATE);
    expect(f.controls.mouseButtons.LEFT).toBe(MOUSE.PAN);
    expect(f.actions.turntable).not.toHaveBeenCalled();
    f.pointer('pointerup', { button: 1 }); expect(f.actions.select).not.toHaveBeenCalled();
  } finally { f.dispose(); }
});

it('owns clicks by pointer and remembers excursions past the threshold after returning', () => {
  const f = inputFixture();
  try {
    f.pointer('pointerdown');
    f.pointer('pointerup', { pointerId: 2 }); expect(f.actions.select).not.toHaveBeenCalled();
    f.pointer('pointermove', { clientX: 25 });
    f.pointer('pointerup'); expect(f.actions.select).not.toHaveBeenCalled();
    f.pointer('pointerdown');
    f.pointer('pointercancel');
    f.pointer('pointerup'); expect(f.actions.select).not.toHaveBeenCalled();
    f.pointer('pointerdown');
    f.pointer('pointerup', { clientX: 24 }); expect(f.actions.select).toHaveBeenCalledOnce();
  } finally { f.dispose(); }
});
