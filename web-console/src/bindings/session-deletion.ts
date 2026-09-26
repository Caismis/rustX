import { message, type Message } from '../locale/translation';
import type { RuntimeClientSessionDeletionResult } from '../../../protocol/app-server/v23';

/** Product copy must preserve committed, blocked and uncertain deletion outcomes. */
export function sessionDeletionNotice(result: RuntimeClientSessionDeletionResult): Message {
  switch (result.status) {
    case 'deleted': return message('common:copy.session-deleted');
    case 'not_found': return message('common:copy.this-session-no-longer-exists');
    case 'stale': return message('common:copy.this-session-changed-review-a-new-deletion-confirmation-before-deleting');
    case 'committed_cleanup_pending': return message('common:copy.deletion-was-committed-but-cleanup-is-still-pending-inspect-the-recorded-details-before-ta');
    case 'committed_durability_uncertain': return message('common:copy.deletion-needs-verification-storage-did-not-confirm-durability-check-saved-work-and-the-re');
    case 'blocked':
      switch (result.reason.kind) {
        case 'resource_conflict': return message('common:copy.an-external-resource-owner-prevents-deletion-inspect-the-ownership-conflict-before-trying-');
        case 'workspace': return message('common:copy.retained-workspaces-are-blocking-deletion-review-the-affected-workspaces-and-recorded-deta');
        case 'invalid_ownership': return message('common:copy.saved-ownership-could-not-be-verified-inspect-the-recorded-details-before-trying-again');
      }
    case 'preview': return message('common:copy.review-the-deletion-confirmation-before-continuing');
  }
}
