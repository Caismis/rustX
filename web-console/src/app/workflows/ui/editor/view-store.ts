/** Only personal preferences live in the browser. The complete Workflow scene
 * has one viewport; native node/Block identities never key browser subgraphs. */
export interface ViewportPreference { x: number; y: number; zoom: number }
const PREFIX = 'rustx-workflow-viewport-v3:';
const WIDTH_KEY = 'rustx-workflow-inspector-width';
export function viewKey(endpoint: string, scope: string, id: string): string { return PREFIX + JSON.stringify([endpoint, scope, id]); }
export function readViewport(key: string): ViewportPreference | undefined {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(key) ?? 'null');
    if (!value || typeof value !== 'object') return undefined;
    const { x, y, zoom } = value as ViewportPreference;
    return [x, y, zoom].every(value => typeof value === 'number' && Number.isFinite(value)) && zoom >= 0.05 && zoom <= 2 ? { x, y, zoom } : undefined;
  } catch { return undefined; }
}
export function writeViewport(key: string, viewport: ViewportPreference): void {
  try { localStorage.setItem(key, JSON.stringify(viewport)); } catch { /* Optional personal preference. */ }
}
export function readInspectorWidth(): number {
  try { const width = Number(localStorage.getItem(WIDTH_KEY)); return width >= 320 && width <= 640 ? width : 392; } catch { return 392; }
}
export function writeInspectorWidth(width: number): void {
  try { localStorage.setItem(WIDTH_KEY, String(width)); } catch { /* Optional personal preference. */ }
}
