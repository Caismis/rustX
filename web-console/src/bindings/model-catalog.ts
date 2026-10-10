import type { ModelCatalogView, SessionModelConfig } from '../../../protocol/app-server/v39';
import type { ModelChoice, ModelSelectionIntent } from '../presentation/agent/ModelSelect';
/** Native catalog order, Model Profiles and default profile, unchanged. */
export function catalogChoices(catalog?: ModelCatalogView): ModelChoice[] {
  return (catalog?.models ?? []).map(value => ({ id: value.model, profiles: (value.profiles ?? []).map(profile => ({ id: profile.id, label: profile.id })), defaultProfile: value.defaultProfile ?? undefined }));
}
/** Whether the native catalog publishes this exact model and optional profile. */
export function catalogAdmits(catalog: ModelCatalogView | undefined, model: string, profile?: string) {
  return !!catalog?.models?.some(value => value.model === model && (profile === undefined || value.profiles?.some(item => item.id === profile)));
}
/** The Profile an intent names; `model` and `model-default` name none. */
export function intentProfile(intent: ModelSelectionIntent) {
  return intent.kind === 'profile' ? intent.profile : undefined;
}

/** Whether a selection gesture changes the configured selection: the authored
 * Model and the pinned Profile, or none. Configured intent is compared, never
 * the effective Profile: pinning the Profile that is currently the Model's
 * default, or returning a pinned default to following the Model, is a change
 * even though the next invocation resolves alike. */
export function selectionChanges(model: string | undefined, pinned: string | undefined, intent: ModelSelectionIntent) {
  if (intent.model !== model) return true;
  switch (intent.kind) {
    case 'model': return false;
    case 'profile': return intent.profile !== pinned;
    case 'model-default': return pinned !== undefined;
  }
}

/** The complete configured selection a gesture produces from the configured
 * one (native `SessionModelConfig` is whole-state), or `undefined` for a
 * genuine no-op.
 *
 * A gesture changes primary-model-owned settings only; the Summary policy is
 * independently owned and always carried unchanged. A Profile gesture on the
 * same Model changes only `profile` and keeps every other setting — request
 * overrides, output limit, explicit Summary; native validation still decides
 * whether they suit the new Profile. Choosing the current Model keeps its
 * pinned Profile. A different Model starts from its own primary defaults — its
 * default Profile unless one is chosen, no request overrides, no output limit —
 * exactly as the TUI's `/model`. */
export function nextModelSelection(configured: SessionModelConfig | undefined, intent: ModelSelectionIntent): SessionModelConfig | undefined {
  const pinned = configured?.profile ?? undefined;
  if (!selectionChanges(configured?.model, pinned, intent)) return undefined;
  const profile = intentProfile(intent);
  if (!configured || configured.model !== intent.model) {
    const summaryModel = configured?.summaryModel;
    return { model: intent.model, ...(profile === undefined ? {} : { profile }), ...(summaryModel === undefined ? {} : { summaryModel }) };
  }
  const { profile: _, ...independent } = configured;
  return profile === undefined ? independent : { ...independent, profile };
}
