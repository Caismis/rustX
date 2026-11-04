import { Vector3, type Camera } from 'three';
import type { StandardView } from './viewport';
import css from './GeometryPreview.module.css';

type AxisView = Exclude<StandardView, 'isometric'>;
export type AxisLabels = Record<AxisView | 'axes', string>;
const endpoints: { view: AxisView; label: string; direction: [number, number, number] }[] = [
  { view: 'right', label: /* i18n-raw: mathematical signed coordinate-axis symbol */ '+X', direction: [1, 0, 0] }, { view: 'left', label: /* i18n-raw: mathematical signed coordinate-axis symbol */ '−X', direction: [-1, 0, 0] },
  { view: 'top', label: /* i18n-raw: mathematical signed coordinate-axis symbol */ '+Y', direction: [0, 1, 0] }, { view: 'bottom', label: /* i18n-raw: mathematical signed coordinate-axis symbol */ '−Y', direction: [0, -1, 0] },
  { view: 'front', label: /* i18n-raw: mathematical signed coordinate-axis symbol */ '+Z', direction: [0, 0, 1] }, { view: 'back', label: /* i18n-raw: mathematical signed coordinate-axis symbol */ '−Z', direction: [0, 0, -1] },
];
// Bounded collision candidates keep pole/opposite endpoints independently usable.
const offsets = [0, -28, 28, -56, 56].flatMap(x => [0, -28, 28, -56, 56].map(y => [x, y]));
offsets.sort((a, b) => a[0] ** 2 + a[1] ** 2 - b[0] ** 2 - b[1] ** 2);

/** Accessible endpoints share the rendered indicator; camera updates stay outside React. */
export function createAxisNavigation(container: HTMLElement, labels: AxisLabels, navigate: (view: StandardView) => void) {
  const ns = 'http://www.w3.org/2000/svg', element = document.createElementNS(ns, 'svg'); element.classList.add(css.axes); element.setAttribute('role', 'group'); element.dataset.geometryAxisIndicator = '';
  const axes = endpoints.map(endpoint => {
    const group = document.createElementNS(ns, 'g'), hit = document.createElementNS(ns, 'path'), label = document.createElementNS(ns, 'text'), title = document.createElementNS(ns, 'title');
    group.classList.add(css.axisControl); group.setAttribute('role', 'button'); group.setAttribute('tabindex', '0'); group.dataset.geometryAxis = endpoint.view;
    group.dataset.axis = endpoint.label.slice(-1); group.dataset.negative = String(endpoint.direction.some(value => value < 0));
    hit.classList.add(css.axisHit); label.classList.add(css.axisLabel); label.textContent = endpoint.label;
    label.setAttribute('text-anchor', 'middle'); label.setAttribute('dominant-baseline', 'central');
    group.addEventListener('pointerdown', event => { event.stopPropagation(); group.focus({ preventScroll: true }); });
    group.addEventListener('click', event => { event.stopPropagation(); navigate(endpoint.view); });
    group.addEventListener('keydown', event => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); event.stopPropagation(); navigate(endpoint.view); } });
    group.append(title, hit, label); element.append(group); return { group, hit, label, title };
  });
  function relabel(value: AxisLabels) {
    element.setAttribute('aria-label', value.axes);
    endpoints.forEach((endpoint, index) => { const name = `${endpoint.label}: ${value[endpoint.view]}`; axes[index].group.setAttribute('aria-label', name); axes[index].title.textContent = name; });
  }
  relabel(labels); container.append(element);
  return {
    relabel,
    update(camera: Camera, side: number) {
      element.style.width = `${side}px`; element.style.height = `${side}px`;
      element.setAttribute('viewBox', `0 0 ${side} ${side}`); element.style.display = side < 108 ? 'none' : '';
      const placed: { x: number; y: number }[] = [];
      const projected = endpoints.map((endpoint, index) => ({ index, point: new Vector3(...endpoint.direction).project(camera) })).sort((a, b) => a.point.z - b.point.z);
      for (const { index, point } of projected) {
        const desiredX = (point.x + 1) * side / 2, desiredY = (1 - point.y) * side / 2;
        let x = 14, y = 14;
        for (const [dx, dy] of offsets) {
          x = Math.min(side - 14, Math.max(14, desiredX + dx)); y = Math.min(side - 14, Math.max(14, desiredY + dy));
          if (placed.every(other => Math.abs(other.x - x) >= 26 || Math.abs(other.y - y) >= 26)) break;
        }
        placed.push({ x, y }); axes[index].label.setAttribute('x', String(x)); axes[index].label.setAttribute('y', String(y));
        // Hit the rendered axis itself, excluding the shared origin. Pole axes
        // remain addressable through their signed labels without separate chrome.
        const dx = desiredX - side / 2, dy = desiredY - side / 2, length = Math.hypot(dx, dy);
        axes[index].hit.setAttribute('d', length > 12 ? `M${side / 2 + dx / length * 10},${side / 2 + dy / length * 10}L${desiredX},${desiredY}` : '');
      }
    },
    dispose: () => element.remove(),
  };
}
