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
  const element = document.createElement('div'); element.className = css.axes; element.setAttribute('role', 'group');
  const buttons = endpoints.map(endpoint => {
    const button = document.createElement('button'); button.type = 'button'; button.className = css.axisButton;
    button.textContent = endpoint.label; button.dataset.geometryAxis = endpoint.view;
    button.dataset.axis = endpoint.label.slice(-1); button.dataset.negative = String(endpoint.direction.some(value => value < 0));
    button.addEventListener('pointerdown', event => event.stopPropagation());
    button.addEventListener('click', event => { event.stopPropagation(); navigate(endpoint.view); });
    element.append(button); return button;
  });
  function relabel(value: AxisLabels) {
    element.setAttribute('aria-label', value.axes);
    endpoints.forEach((endpoint, index) => { const name = `${endpoint.label}: ${value[endpoint.view]}`; buttons[index].setAttribute('aria-label', name); buttons[index].title = name; });
  }
  relabel(labels); container.append(element);
  return {
    relabel,
    update(camera: Camera, side: number) {
      element.style.width = `${side}px`; element.style.height = `${side}px`;
      element.hidden = side < 108;
      const placed: { x: number; y: number }[] = [];
      const projected = endpoints.map((endpoint, index) => ({ index, point: new Vector3(...endpoint.direction).project(camera) })).sort((a, b) => a.point.z - b.point.z);
      for (const { index, point } of projected) {
        const desiredX = (point.x + 1) * side / 2, desiredY = (1 - point.y) * side / 2;
        let x = 14, y = 14;
        for (const [dx, dy] of offsets) {
          x = Math.min(side - 14, Math.max(14, desiredX + dx)); y = Math.min(side - 14, Math.max(14, desiredY + dy));
          if (placed.every(other => Math.abs(other.x - x) >= 26 || Math.abs(other.y - y) >= 26)) break;
        }
        placed.push({ x, y }); buttons[index].style.transform = `translate(${x}px, ${y}px) translate(-50%, -50%)`;
      }
    },
    dispose: () => element.remove(),
  };
}
