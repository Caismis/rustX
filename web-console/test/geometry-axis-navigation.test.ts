import { afterEach, expect, it, vi } from 'vitest';
import { PerspectiveCamera, Vector3 } from 'three';
import { createAxisNavigation } from '../src/app/components/documents/geometry/axes';
import { en, zh } from '../src/locale/dictionaries/geometry';

afterEach(() => document.body.replaceChildren());
function camera(position: number[]) {
  const camera = new PerspectiveCamera(45, 1, .01, 10);
  camera.position.fromArray(position).normalize().multiplyScalar(3.5);
  if (Math.abs(position[1]) === Math.hypot(...position)) camera.up.set(0, 0, position[1] > 0 ? -1 : 1);
  camera.lookAt(0, 0, 0); camera.updateMatrixWorld(); return camera;
}
function mount() {
  const container = document.createElement('div'); document.body.append(container);
  const navigate = vi.fn(), navigation = createAxisNavigation(container, en, navigate);
  const group = (axis: string) => container.querySelector<SVGGElement>(`[data-axis="${axis}"]`)!;
  const click = (axis: string) => group(axis).dispatchEvent(new MouseEvent('click', { bubbles: true }));
  return { container, navigate, navigation, group, click };
}
function tips(container: HTMLElement) {
  return [...container.querySelectorAll('[data-axis]')].map(group => {
    const hit = group.querySelectorAll('circle')[1];
    return { x: Number(hit.getAttribute('cx')), y: Number(hit.getAttribute('cy')), radius: Number(hit.getAttribute('r')) };
  });
}

it('draws only three arrows without signed text, with 14px heads and 24px pointer targets', () => {
  const f = mount(); f.navigation.update(camera([1, 1, 1]), 144);
  expect(f.container.querySelectorAll('[role="button"]')).toHaveLength(3);
  expect(f.container.querySelectorAll('text')).toHaveLength(0);
  expect([...f.container.querySelectorAll('[data-axis]')].map(group => group.getAttribute('data-axis'))).toEqual(['X', 'Y', 'Z']);
  for (const group of f.container.querySelectorAll('[data-axis]')) {
    const points = group.querySelectorAll('path')[1].getAttribute('d')!.match(/-?\d+(?:\.\d+)?(?:e[+-]?\d+)?/g)!.map(Number);
    const [x, y, a, b, c, d] = points;
    expect(Math.hypot(x - (a + c) / 2, y - (b + d) / 2)).toBeCloseTo(14);
    expect(Math.hypot(a - c, b - d)).toBeCloseTo(14);
  }
  for (const tip of tips(f.container)) expect(tip.radius * 2).toBeGreaterThanOrEqual(24);
});

it.each([['X', 'right', 'left'], ['Y', 'top', 'bottom'], ['Z', 'front', 'back']])('activates %s positive then opposite, including two activations before the next render', (axis, positive, negative) => {
  const f = mount(); f.navigation.update(camera([1, 1, 1]), 144);
  f.click(axis); expect(f.navigate).toHaveBeenLastCalledWith(positive);
  expect(f.group(axis).getAttribute('aria-label')).toBe(en[negative as keyof typeof en]);
  f.click(axis); expect(f.navigate).toHaveBeenLastCalledWith(negative);
  expect(f.group(axis).getAttribute('aria-label')).toBe(en[positive as keyof typeof en]);
});

it.each([
  ['X', [1, 0, 0], 'left'], ['X', [-1, 0, 0], 'right'],
  ['Y', [0, 1, 0], 'bottom'], ['Y', [0, -1, 0], 'top'],
  ['Z', [0, 0, 1], 'back'], ['Z', [0, 0, -1], 'front'],
] as const)('reads the aligned camera for %s %j instead of retaining an old click counter', (axis, position, next) => {
  const f = mount(); f.navigation.update(camera([...position]), 144);
  f.click(axis); expect(f.navigate).toHaveBeenLastCalledWith(next);
  f.navigation.update(camera([2, 1, 3]), 144); f.click(axis);
  expect(f.navigate).toHaveBeenLastCalledWith(axis === 'X' ? 'right' : axis === 'Y' ? 'top' : 'front');
});

it('keeps every hit target separate and bounded at poles, opposite views, and collinear projections', () => {
  const f = mount();
  for (const side of [108, 144]) for (const position of [[1,0,0], [-1,0,0], [0,1,0], [0,-1,0], [0,0,1], [0,0,-1], [1,-1,0], [-1,1,0], [1,0,-1], [0,1,-1]]) {
    f.navigation.update(camera(position), side); const points = tips(f.container);
    for (const point of points) {
      expect(point.x - point.radius).toBeGreaterThanOrEqual(0); expect(point.x + point.radius).toBeLessThanOrEqual(side);
      expect(point.y - point.radius).toBeGreaterThanOrEqual(0); expect(point.y + point.radius).toBeLessThanOrEqual(side);
    }
    for (let i = 0; i < points.length; i++) for (let j = i + 1; j < points.length; j++) expect(Math.hypot(points[i].x - points[j].x, points[i].y - points[j].y)).toBeGreaterThanOrEqual(24);
    if (Math.abs(position[1]) === 1 && position[0] === 0 && position[2] === 0) {
      expect(points[1]).toEqual({ x: side / 2, y: side / 2, radius: 12 });
      expect(f.group('Y').querySelector('circle')!.style.display).toBe('');
      f.click('Y'); expect(f.navigate).toHaveBeenLastCalledWith(position[1] > 0 ? 'bottom' : 'top');
    }
  }
  f.navigation.update(camera([1, 1, 1]), 107); expect(f.container.querySelector('svg')!.style.display).toBe('none');
});

it('preserves projected directions across camera orientations while separating the three targets', () => {
  const f = mount();
  for (let x = -2; x <= 2; x++) for (let y = -2; y <= 2; y++) for (let z = -2; z <= 2; z++) {
    if (!x && !y && !z) continue;
    const view = camera([x, y, z]); f.navigation.update(view, 108); const points = tips(f.container), inverse = view.quaternion.clone().invert();
    for (const [index, direction] of [[1,0,0], [0,1,0], [0,0,1]].entries()) {
      const projected = new Vector3(...direction).applyQuaternion(inverse), point = points[index];
      expect((point.x - 54) * -projected.y - (point.y - 54) * projected.x).toBeCloseTo(0, 7);
    }
    for (let i = 0; i < points.length; i++) for (let j = i + 1; j < points.length; j++) expect(Math.hypot(points[i].x - points[j].x, points[i].y - points[j].y)).toBeGreaterThanOrEqual(24);
  }
});

it('supports Enter and Space, updates localized action names, and retires its controls', () => {
  const f = mount(), bubbled = vi.fn(); f.container.addEventListener('keydown', bubbled);
  f.navigation.update(camera([1, 1, 1]), 144);
  for (const key of ['Enter', ' ']) {
    const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true }); f.group('X').dispatchEvent(event); expect(event.defaultPrevented).toBe(true);
  }
  expect(f.navigate.mock.calls).toEqual([['right'], ['left']]); expect(bubbled).not.toHaveBeenCalled();
  f.navigation.relabel(zh); expect(f.group('X').getAttribute('aria-label')).toBe(zh.right); expect(f.group('X').querySelector('title')!.textContent).toBe(zh.right);
  expect(f.container.querySelector('svg')!.getAttribute('aria-label')).toBe(zh.axes);
  const removed = f.group('X'); f.navigation.dispose(); expect(f.container.children).toHaveLength(0);
  removed.dispatchEvent(new MouseEvent('click')); expect(f.navigate).toHaveBeenCalledTimes(2);
});
