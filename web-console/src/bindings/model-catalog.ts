import type { ModelCatalogView } from '../../../protocol/app-server/v44';
import type { ModelChoice } from '../presentation/agent/ModelSelect';
/** Native catalog order, reasoning profiles and default profile, unchanged. */
export function catalogChoices(catalog?: ModelCatalogView): ModelChoice[] {
  return (catalog?.models ?? []).map(value => ({ id: value.model, profiles: (value.reasoningProfiles ?? []).map(profile => ({ id: profile.id, label: profile.id })), defaultProfile: value.defaultReasoningProfile ?? undefined }));
}
/** Whether the native catalog publishes this exact model and optional profile. */
export function catalogAdmits(catalog: ModelCatalogView | undefined, model: string, profile?: string) {
  return !!catalog?.models?.some(value => value.model === model && (profile === undefined || value.reasoningProfiles?.some(item => item.id === profile)));
}

/** Equivalent model selection preserves the configured override; a different
 * model uses native defaults unless an exact profile is explicitly selected. */
export function modelSelectionChanges(choices: readonly ModelChoice[], current: string | undefined, profile: string | undefined, model: string, selectedProfile?: string) {
  return model !== current || selectedProfile !== undefined && selectedProfile !== (profile ?? choices.find(choice => choice.id === current)?.defaultProfile);
}
