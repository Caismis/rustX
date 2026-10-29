import type { BlockPath, BlockStep } from './model';
import type { WorkflowDiagnostic } from '../../../../../protocol/app-server/v62';

/** Retained orphan diagnostics do not invalidate an executable or block copy. */
export function presentationWarning(diagnostic: WorkflowDiagnostic | null | undefined): boolean {
  return diagnostic?.category === 'presentation_reference' || diagnostic?.category === 'presentation_truncated';
}

/** Where a native diagnostic path points in the editing document. */
export interface DiagnosticLocation {
  /** The lexical block that owns the failing element. */
  readonly block: BlockPath;
  /** The failing node of that block, when the path names one. */
  readonly node?: string;
  /** The failing edge index of that block, when the path names one. */
  readonly edge?: number;
  /** The remaining native field path below the node, edge or block. */
  readonly field?: string;
}

/** Locate a native authored path (`block.nodes.r.body.edges.0.to`,
 * `description`, `$`). Presentation only: the path itself is native. */
export function locate(path: string): DiagnosticLocation {
  const segments = path.split('.');
  if (segments[0] !== 'block') return { block: [], field: path === '$' ? undefined : path };
  const block: BlockStep[] = [];
  let index = 1;
  for (;;) {
    const segment = segments[index];
    if (segment === undefined) return { block };
    if (segment === 'edges') {
      const edge = Number(segments[index + 1]);
      return Number.isSafeInteger(edge)
        ? { block, edge, field: segments.slice(index + 2).join('.') || undefined }
        : { block, field: 'edges' };
    }
    if (segment !== 'nodes' || segments[index + 1] === undefined) {
      return { block, field: segments.slice(index).join('.') };
    }
    const node = segments[index + 1];
    index += 2;
    if (segments[index] === 'body' && (segments[index + 1] === undefined || ['nodes', 'edges', 'input', 'output'].includes(segments[index + 1]))) {
      if (segments[index + 1] === undefined) return { block, node, field: 'body' };
      block.push({ node });
      index += 1;
      continue;
    }

    return { block, node, field: segments.slice(index).join('.') || undefined };
  }
}
