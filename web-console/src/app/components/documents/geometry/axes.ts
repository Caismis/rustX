import { Quaternion, Vector3, type Camera } from 'three';
import type { StandardView } from './viewport';
import css from './GeometryPreview.module.css';

export type GeometryAxis = 'X' | 'Y' | 'Z';
type AxisView = Exclude<StandardView, 'isometric'>;
export type AxisLabels = Record<AxisView | 'axes', string>;
const directions: { axis: GeometryAxis; positive: AxisView; negative: AxisView; direction: [number, number, number] }[] = [
  { axis: 'X', positive: 'right', negative: 'left', direction: [1, 0, 0] },
  { axis: 'Y', positive: 'top', negative: 'bottom', direction: [0, 1, 0] },
  { axis: 'Z', positive: 'front', negative: 'back', direction: [0, 0, 1] },
];
const hitRadius = 12, headLength = 14, headHalfWidth = 7;

/** The three projected arrows are both the orientation indicator and its controls. */
export function createAxisNavigation(container: HTMLElement, labels: AxisLabels, navigate: (view: StandardView) => void) {
  const ns = 'http://www.w3.org/2000/svg', element = document.createElementNS(ns, 'svg');
  element.classList.add(css.axes); element.setAttribute('role', 'group'); element.dataset.geometryAxisIndicator = '';
  let disposed = false;
  const axes = directions.map(endpoint => {
    const group = document.createElementNS(ns, 'g'), shaft = document.createElementNS(ns, 'path'), head = document.createElementNS(ns, 'path');
    const pole = document.createElementNS(ns, 'circle'), hit = document.createElementNS(ns, 'circle'), title = document.createElementNS(ns, 'title');
    group.classList.add(css.axisControl); group.setAttribute('role', 'button'); group.setAttribute('tabindex', '0'); group.dataset.axis = endpoint.axis;
    shaft.classList.add(css.axisShaft); head.classList.add(css.axisHead); pole.classList.add(css.axisPole); hit.classList.add(css.axisHit);
    pole.setAttribute('r', String(headHalfWidth)); hit.setAttribute('r', String(hitRadius));
    const control = { ...endpoint, group, shaft, head, pole, hit, title, next: endpoint.positive };
    const activate = () => {
      if (disposed) return;
      const view = control.next;
      // Advance before navigation so two activations before the next frame flip.
      control.next = view === endpoint.positive ? endpoint.negative : endpoint.positive;
      name(control); navigate(view);
    };
    group.addEventListener('pointerdown', event => { event.stopPropagation(); group.focus({ preventScroll: true }); });
    group.addEventListener('click', event => { event.stopPropagation(); activate(); });
    group.addEventListener('keydown', event => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); event.stopPropagation(); activate(); } });
    group.append(title, shaft, head, pole, hit); element.append(group); return control;
  });
  function name(control: typeof axes[number]) {
    control.group.dataset.geometryAxis = control.next;
    control.group.setAttribute('aria-label', labels[control.next]); control.title.textContent = labels[control.next];
  }
  function relabel(value: AxisLabels) {
    labels = value; element.setAttribute('aria-label', labels.axes); axes.forEach(name);
  }
  relabel(labels); container.append(element);
  return {
    relabel,
    get focusedAxis() { return disposed ? undefined : axes.find(control => control.group === container.ownerDocument.activeElement)?.axis; },
    focusAxis(axis: GeometryAxis) { if (!disposed) axes.find(control => control.axis === axis)?.group.focus({ preventScroll: true }); },
    update(camera: Camera, side: number) {
      element.style.width = `${side}px`; element.style.height = `${side}px`;
      element.setAttribute('viewBox', `0 0 ${side} ${side}`); element.style.display = side < 108 ? 'none' : '';
      const center = side / 2, radius = Math.max(14, center - 14), rotation = camera.getWorldQuaternion(new Quaternion());
      const facing = new Vector3(0, 0, 1).applyQuaternion(rotation), inverse = rotation.invert();
      const projected = axes.map(control => {
        const direction = new Vector3(...control.direction), point = direction.clone().applyQuaternion(inverse);
        control.next = facing.dot(direction) > 1 - 1e-8 ? control.negative : control.positive; name(control);
        const magnitude = Math.hypot(point.x, point.y), pole = magnitude * radius < 1;
        return { control, pole, ux: pole ? 0 : point.x / magnitude, uy: pole ? 0 : -point.y / magnitude, length: magnitude * radius };
      }).sort((a, b) => a.length - b.length);
      const placed: { x: number; y: number }[] = [];
      // At most three lengths per axis suffice to separate collinear projections.
      // Only the length changes: each arrow retains its true screen direction.
      function place(index: number): boolean {
        if (index === projected.length) return true;
        const { pole, ux, uy, length } = projected[index];
        for (const distance of pole ? [0] : [Math.max(26, length), radius, 14]) {
          const point = { x: ux * distance, y: uy * distance };
          if (placed.some(other => Math.hypot(point.x - other.x, point.y - other.y) < 26 - 1e-6)) continue;
          placed.push(point);
          if (place(index + 1)) return true;
          placed.pop();
        }
        return false;
      }
      place(0);
      for (const [index, { control, pole, ux, uy, length }] of projected.entries()) {
        const point = placed[index] ?? { x: ux * length, y: uy * length }, x = center + point.x, y = center + point.y;
        for (const circle of [control.hit, control.pole]) { circle.setAttribute('cx', String(x)); circle.setAttribute('cy', String(y)); }
        control.pole.style.display = pole ? '' : 'none'; control.shaft.style.display = control.head.style.display = pole ? 'none' : '';
        const baseX = x - ux * headLength, baseY = y - uy * headLength;
        control.shaft.setAttribute('d', `M${center},${center}L${baseX},${baseY}`);
        control.head.setAttribute('d', `M${x},${y}L${baseX - uy * headHalfWidth},${baseY + ux * headHalfWidth}L${baseX + uy * headHalfWidth},${baseY - ux * headHalfWidth}Z`);
      }
    },
    dispose() { disposed = true; element.remove(); },
  };
}
