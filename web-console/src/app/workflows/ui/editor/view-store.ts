import { blockKey } from '../../document/model';
import type { IdentityRename } from '../../machines/editor';

/** Personal viewport preferences of one exact Workflow file and Block. It holds no program data —
 * no task, argument, value, schema or secret — and never syncs. Missing or
 * unreadable entries fall back to the deterministic default layout. */
export interface BlockView {
  viewport?: { x: number; y: number; zoom: number };
}

const PREFIX = 'rustx-workflow-viewport-v2:';
const MAX_BLOCKS = 64;

function finite(value: unknown): value is number { return typeof value === 'number' && Number.isFinite(value); }

function sanitize(value: unknown): Record<string, BlockView> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return {};
  const result: [string, BlockView][] = [];
  for (const [block, view] of Object.entries(value).slice(0, MAX_BLOCKS)) {
    if (!view || typeof view !== 'object') continue;
    const entry: BlockView = {};
    const { viewport } = view as BlockView;
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

/** Move only browser presentation at an accepted identity boundary. The
 * inverse uses the same operation, so undo/redo carry current viewports too.
 * Old names are removed instead of accumulating aliases in the bounded cache. */
export function renameViews(key: string, { from, to }: IdentityRename) {
  try {
    const parent = blockKey(from.block), prefix = parent === 'root' ? '' : `${parent}/`;
    const branch = from.path.length !== 0;
    const oldRoot = `${prefix}${from.node}${branch ? `/${(from.path[1] as { key: string }).key}` : ''}`;
    const newRoot = `${prefix}${to.node}${branch ? `/${(to.path[1] as { key: string }).key}` : ''}`;
    const within = (block: string, root: string) => (branch && block === root) || block.startsWith(`${root}/`);
    const views: [string, BlockView][] = [];
    for (const [block, stored] of Object.entries(readViews(key))) {
      // The destination identity did not exist in the accepted document.
      // Its old, orphaned preferences cannot replace the moving subtree.
      if (within(block, newRoot) && !within(block, oldRoot)) continue;
      views.push([within(block, oldRoot) ? newRoot + block.slice(oldRoot.length) : block, stored]);
    }
    localStorage.setItem(key, JSON.stringify(sanitize(Object.fromEntries(views))));
  } catch { /* Presentation preferences are optional. */ }
}
