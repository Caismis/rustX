import { useEffect, useRef, useState } from 'react';
import type { WorkflowModelPolicy, WorkflowModelChoices, WorkflowModelChoicesQuery } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { encodeField, type FieldAddress, type FieldStep } from '../../../document/fields';
import type { WorkflowScopePort } from '../../../port';
import { JsonField, NumberField, Section, SelectField } from './fields';
import css from '../Editor.module.css';

type Query = { type: 'models' } | { type: 'profiles'; model: string };
interface ChoiceOwner { readonly port: WorkflowScopePort; readonly key: string; active: boolean; request: number }
interface ChoiceState { owner: ChoiceOwner; offset: number; loading: boolean; choices?: WorkflowModelChoices; failed?: boolean }

/** One explicit page of registered choices, never an assembled catalog. Each
 * request belongs to this source port and query; changed source/model owners
 * and unmounted inspectors cannot accept an older completion. */
export function useModelChoices(port: WorkflowScopePort, query: Query | undefined) {
  const key = JSON.stringify(query ?? null);
  const lifetime = useRef<ChoiceOwner | undefined>(undefined);
  const [state, setState] = useState<ChoiceState>();
  const shown = state?.owner.port === port && state.owner.key === key ? state : undefined;
  const load = (offset = 0, revision?: string) => {
    const owner = lifetime.current;
    if (!query || !owner?.active || owner.port !== port || owner.key !== key) return;
    const request = ++owner.request;
    setState({ owner, offset, loading: true });
    const expected: WorkflowModelChoicesQuery = { ...query, offset };
    void port.modelChoices(expected, revision).then(choices => {
      if (!owner.active || owner.request !== request) return;
      const values = choices.type === 'models' ? choices.models : choices.profiles;
      if (choices.type !== query.type || choices.type === 'profiles' && query.type === 'profiles' && choices.model !== query.model
        || values.length > 32 || values.some(value => value.length > 256)
        || revision !== undefined && choices.revision !== revision
        || choices.next_offset != null && (values.length === 0 || choices.next_offset !== offset + values.length)) {
        throw new Error('Model choices do not match their bounded source query.');
      }
      setState({ owner, offset, loading: false, choices });
    }).catch(() => {
      if (owner.active && owner.request === request) setState({ owner, offset, loading: false, failed: true });
    });
  };
  useEffect(() => {
    const owner: ChoiceOwner = { port, key, active: true, request: 0 };
    lifetime.current = owner;
    if (query) load();
    return () => { owner.active = false; owner.request++; };
  }, [port, key]);
  return { state: shown, load };
}

/** The existing Auxiliary model policy, authored against bounded native
 * registered choices. An unregistered authored identity stays inspectable. */
export function ModelSelection({ policy, address, port, onChange }: {
  policy: WorkflowModelPolicy; address: FieldAddress; port: WorkflowScopePort;
  onChange: (value: WorkflowModelPolicy, field: readonly FieldStep[]) => void;
}) {
  const tx = useTranslation();
  const selected = policy.mode === 'explicit' ? policy : undefined;
  const models = useModelChoices(port, selected ? { type: 'models' } : undefined);
  const profiles = useModelChoices(port, selected?.model ? { type: 'profiles', model: selected.model } : undefined);
  const modelValues = models.state?.choices?.type === 'models' ? models.state.choices.models : [];
  const profileValues = profiles.state?.choices?.type === 'profiles' ? profiles.state.choices.profiles : [];
  const withSelected = (values: readonly string[], selected?: string | null) => [...new Set([...values, ...(selected ? [selected] : [])])];
  return <Section title={tx('workflows:inspector.model-selection')}>
    <SelectField field={encodeField(address)} label={tx('workflows:inspector.model-source')} value={policy.mode}
      options={[{ value: 'session', label: tx('workflows:inspector.model-session') }, { value: 'explicit', label: tx('workflows:inspector.model-explicit') }]}
      hint={tx('workflows:inspector.model-source-hint')}
      onChange={mode => { if (mode !== policy.mode) onChange(mode === 'session' ? { mode } : { mode, model: '', request_params: {} }, []); }} />
    {selected && <>
      <SelectField field={encodeField({ ...address, path: [...address.path, 'model'] })} label={tx('workflows:inspector.model-id')} value={selected.model} disabled={models.state?.loading}
        options={[{ value: '', label: tx('workflows:inspector.model-choose') }, ...withSelected(modelValues, selected.model).map(model => ({ value: model, label: model }))]}
        onChange={model => onChange({ ...selected, model, profile: undefined }, ['model'])} />
      <ChoicePages choices={models} kind="models" />
      <SelectField label={tx('workflows:inspector.model-profile')} value={selected.profile ?? ''} disabled={profiles.state?.loading}
        options={[{ value: '', label: tx('workflows:inspector.model-default-profile') }, ...withSelected(profileValues, selected.profile).map(profile => ({ value: profile, label: profile }))]}
        onChange={profile => onChange({ ...selected, profile: profile || undefined }, ['profile'])} />
      {selected.model && <ChoicePages choices={profiles} kind="profiles" />}
      <details className={css.advanced}><summary>{tx('workflows:panel.advanced')}</summary>
      <NumberField label={tx('workflows:inspector.model-output-limit')} hint={tx('workflows:inspector.model-output-limit-hint')} value={selected.max_output_tokens} min={1} max={4294967295} optional address={{ ...address, path: [...address.path, 'max_output_tokens'] }} />
      <JsonField label={tx('workflows:inspector.model-params')} hint={tx('workflows:inspector.model-params-hint')} value={selected.request_params ?? {}} rows={5} address={{ ...address, path: [...address.path, 'request_params'], role: 'optional' }} />
      </details>
    </>}
  </Section>;
}

function ChoicePages({ choices: { state, load }, kind }: { choices: ReturnType<typeof useModelChoices>; kind: 'models' | 'profiles' }) {
  const tx = useTranslation();
  if (state?.loading) return <p className={css.note} role="status">{tx('workflows:inspector.model-choices-loading')}</p>;
  if (state?.failed) return <div><p className={css.note} role="alert">{tx('workflows:inspector.model-choices-failed')}</p>
    <button type="button" className={css.smallButton} onClick={() => load()}>{tx('workflows:inspector.model-choices-reload')}</button></div>;
  if (!state?.choices) return null;
  const { revision, next_offset } = state.choices;
  return <div className={css.inline}>
    {state.offset > 0 && <button type="button" className={css.smallButton} onClick={() => load(Math.max(0, state.offset - 32), revision)}>{tx(kind === 'models' ? 'workflows:inspector.model-choices-previous-models' : 'workflows:inspector.model-choices-previous-profiles')}</button>}
    {next_offset != null && <button type="button" className={css.smallButton} onClick={() => load(next_offset, revision)}>{tx(kind === 'models' ? 'workflows:inspector.model-choices-more-models' : 'workflows:inspector.model-choices-more-profiles')}</button>}
  </div>;
}
