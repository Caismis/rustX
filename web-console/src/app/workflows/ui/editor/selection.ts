import type { PastedFragment } from '../../document/clipboard';
import type { BlockPath } from '../../document/model';

/** Selection names a native lexical owner. A view id never becomes a node id. */
export type Selection =
  | { kind: 'node'; id: string; path?: BlockPath }
  | { kind: 'edge'; index: number; path?: BlockPath }
  | { kind: 'start'; path: BlockPath }
  | { kind: 'note'; id: string; path: BlockPath }
  | { kind: 'block'; path?: BlockPath }
  | { kind: 'check' }
  | undefined;

export function viewId(path: BlockPath, kind: 'node' | 'start' | 'lane' | 'note' | 'edge' | 'entry', id = ''): string {
  // XYFlow interpolates view IDs into double-quoted DOM attribute selectors.
  // Encode the complete structured identity so quotes cannot break hit testing.
  return encodeURIComponent(JSON.stringify([path.map(step => step.branch === undefined ? [step.node] : [step.node, step.branch]), kind, id]));
}

export type Duplicate = (path: BlockPath, ids: readonly string[], notes: readonly string[]) => PastedFragment | undefined;
