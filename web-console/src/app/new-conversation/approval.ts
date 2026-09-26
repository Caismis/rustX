import type { TranslationKey } from '../../locale/translation';
import type { ApprovalMode } from '../../../../protocol/app-server/v23';
import { admitsSourceMutation, unitOutcome } from '../settings/machines/settings-target';
import type { SettingsTargetActor } from '../settings/machines/system';
import { awaitingCommitObservation, requiresReview } from '../settings/machines/unit-transaction';

export const approvalMutation = (authored: ApprovalMode | null) => ({ kind: 'config' as const, mutation: { unit: 'approval' as const, authored } });
export const approvalIdentity = JSON.stringify(approvalMutation(null));

/** Read the existing transaction at the submission boundary, not a React render's
 * cached boolean. No queue, retry, CAS or mutation ownership lives here.
 * New Session composition resolves current canonical sources (composition.rs /
 * UserConfigManager::resolve_session); Attempt admission freezes that runtime's
 * effective approval. Therefore the post-commit source observation is required,
 * not application to an unrelated already-resident Session. */
export function workspaceApprovalBlock(actor: SettingsTargetActor): TranslationKey | undefined {
  const target = actor.getSnapshot();
  const unit = target.context.units[approvalIdentity]?.getSnapshot();
  const outcome = unitOutcome(target, approvalIdentity);
  if (unit?.matches({ mutation: 'submitting' })) return 'workspace:approval.applying';
  if ((unit && awaitingCommitObservation(unit)) || outcome.kind === 'committed' || (outcome.kind === 'saved' && !outcome.observed)) return 'workspace:approval.saved';
  if (outcome.kind === 'uncertain') return 'workspace:approval.uncertain';
  if (outcome.kind === 'conflict' || (unit && requiresReview(unit))) return 'workspace:approval.conflict';
  if (unit?.context.draft) return 'workspace:approval.draft';
  if (!admitsSourceMutation(target.context) || !target.context.observation?.prospective_approval_mode) return 'workspace:approval.reading';
}
