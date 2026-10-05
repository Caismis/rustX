import type { SessionFileReference } from '../../protocol/app-server/v35.ts';

/** Stale-source comparison only. Equality never grants authority to read a file. */
export function sameSessionFile(a: SessionFileReference | undefined, b: SessionFileReference | undefined): boolean {
  if (!a || !b) return a === b;
  return a.scope.conversation_id === b.scope.conversation_id
    && a.scope.device === b.scope.device && a.scope.inode === b.scope.inode
    && a.path === b.path && a.name === b.name && a.mime_type === b.mime_type
    && (a.description ?? null) === (b.description ?? null);
}
