import type { Point } from './layout';

/** Browser-local canvas presentation of one exact Workflow file: node
 * coordinates and viewport per lexical block. It holds no program data —
 * no task, argument, value, schema or secret — and never syncs. Missing or
 * unreadable entries fall back to the deterministic default layout. */
export interface BlockView {
  positions?: Record<string, Point>;
  viewport?: { x: number; y: number; zoom: number };
}

const PREFIX = 'rustx-workflow-view-v1:';
const MAX_BLOCKS = 64;
const MAX_NODES = 512;

function finite(value: unknown): value is number { return typeof value === 'number' && Number.isFinite(value); }

function sanitize(value: unknown): Record<string, BlockView> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const result: [string, BlockView][] = [];
  for (const [block, view] of Object.entries(value).slice(0, MAX_BLOCKS)) {
    if (!view || typeof view !== 'object') continue;
    const entry: BlockView = {};
    const { positions, viewport } = view as BlockView;
    if (positions && typeof positions === 'object') {
      entry.positions = Object.fromEntries(Object.entries(positions).slice(0, MAX_NODES)
        .filter(([, point]) => point && finite(point.x) && finite(point.y)).map(([id, point]) => [id, { x: point.x, y: point.y }]));
    }
    if (viewport && finite(viewport.x) && finite(viewport.y) && finite(viewport.zoom)) entry.viewport = { x: viewport.x, y: viewport.y, zoom: viewport.zoom };
    result.push([block, entry]);
  }
  // Own entries only, whatever a stored key is named.
  return Object.fromEntries(result);
}

/** The view key of one exact file: server endpoint, scope and id. */
export function viewKey(endpoint: string, scope: string, id: string) {
  return PREFIX + JSON.stringify([endpoint, scope, id]);
}

/** The stored view of one block, by own entry only. */
export function blockView(views: Readonly<Record<string, BlockView>>, block: string): BlockView | undefined {
  return Object.hasOwn(views, block) ? views[block] : undefined;
}

export function readViews(key: string): Record<string, BlockView> {
  try { return sanitize(JSON.parse(localStorage.getItem(key) ?? 'null')); } catch { return {}; }
}

export function writeBlockView(key: string, block: string, update: (view: BlockView) => BlockView) {
  try {
    const views = readViews(key);
    localStorage.setItem(key, JSON.stringify(sanitize({ ...views, [block]: update(blockView(views, block) ?? {}) })));
  } catch { /* Presentation preferences are optional. */ }
}

export function clearViews(key: string) {
  try { localStorage.removeItem(key); } catch { /* Presentation preferences are optional. */ }
}
