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
  return JSON.stringify([path.map(step => step.branch === undefined ? [step.node] : [step.node, step.branch]), kind, id]);
}
