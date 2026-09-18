import type { RuntimeClientSessionDeletionResult } from '../../../protocol/app-server/v6';

/** Product copy must preserve committed, blocked and uncertain deletion outcomes. */
export function sessionDeletionNotice(result: RuntimeClientSessionDeletionResult): string {
  switch (result.status) {
    case 'deleted': return 'Session deleted.';
    case 'not_found': return 'This Session no longer exists.';
    case 'stale': return 'This Session changed. Review a new deletion confirmation before deleting.';
    case 'committed_cleanup_pending': return 'Deletion was committed, but cleanup is still pending. Inspect the recorded details before taking further action.';
    case 'committed_durability_uncertain': return 'Deletion needs verification. Storage did not confirm durability. Check saved work and the recorded details before taking further action.';
    case 'blocked':
      switch (result.reason.kind) {
        case 'current_session': return 'The current Session cannot be deleted. Select another Session before trying again.';
        case 'in_use': return 'This Session is in use. Review Advanced Session controls before deleting.';
        case 'workspace': return 'Retained workspaces are blocking deletion. Review the affected workspaces and recorded details before trying again.';
        case 'invalid_ownership': return 'Saved ownership could not be verified. Inspect the recorded details before trying again.';
      }
    case 'preview': return 'Review the deletion confirmation before continuing.';
  }
}
