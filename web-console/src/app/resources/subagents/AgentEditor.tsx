/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import type {AgentProfileDocument,AgentSkillSelection,SourceScope,SourceSettings,SourceToolSelection} from '../../../../../protocol/app-server/v77';
import css from '../../../presentation/settings/SettingsContent.module.css';
import {useTranslation} from '../../../locale/react';
import {Button} from '../../../presentation/primitives/Button';
import {TypedUnitForm} from '../../settings/forms/bridge';
import {CheckboxList,Names,Selection,nativeTools} from '../../settings/forms/controls';
import {Text} from '../../settings/forms/fields';
import {Advanced} from '../../settings/primitives/aria';
import {ModelSelection,type ModelProfiles} from '../../settings/models/ModelsPage';
export function AgentEditor({ source, scope, name, models, profiles }: {source: SourceSettings; scope: SourceScope; name: string; models: string[]; profiles: ModelProfiles}) {
  const tx = useTranslation();
  const current = source.agents.find(agent => agent.scope === scope && agent.name === name);
  return <section aria-label={tx('settings:extension-detail.agent-definition')}><h4>{tx('settings:extension-detail.agent-definition')}</h4>
    {current?.source.diagnostic && <p role="alert" className={css.error}>{current.source.diagnostic}</p>}
    <TypedUnitForm<AgentProfileDocument> key={`agent:${name}`} title={tx('settings:extension-detail.agent-value', { p0: name })}
      authoredPresent={current !== undefined} authored={current?.source.authored ?? undefined} blank={{}}
      revision={current?.source.revision ?? source.absent_resource_revision}
      removalNotice={<p>{tx('settings:extension-detail.the-root-agent-s-delegation-allowlist-is-a-separate-unit-and-is')}</p>}
      mutation={value => ({ kind: 'agent', name, authored: value })}>
      {form => <AgentFields form={form} models={models} profiles={profiles} />}
    </TypedUnitForm>
  </section>;
}

function AgentFields({ form, models, profiles }: { form: import('../../settings/forms/bridge').TypedUnitForm<AgentProfileDocument>; models: string[]; profiles: ModelProfiles }) {
  const tx = useTranslation();
  const Field = form.Field as unknown as (props: { name: string; children: (field: { state: { value: unknown }; handleChange: (value: never) => void }) => React.ReactNode }) => React.ReactNode;
  return <>
    <Text form={form} name="description" label={tx('settings:extension-detail.description')} />
    <label>{tx('settings:agent-page.instructions')}<Field name="instructions">{field => <textarea value={(field.state.value as string | undefined) ?? ''}
      onChange={event => field.handleChange(event.target.value as never)} />}</Field></label>
    <Field name="model">{field => {
      const model = field.state.value as AgentProfileDocument['model'];
      return <>
        <label><input type="checkbox" checked={!!model} onChange={event => field.handleChange((event.target.checked ? {} : null) as never)} />{tx('settings:extension-detail.explicit-child-model')}</label>
        {model ? <ModelSelection value={model} change={next => field.handleChange(next as never)} models={models} profiles={profiles} />
          : <p>{tx('settings:extension-detail.inherit-the-invoking-attempt-s-already-frozen-effective-model')}</p>}
      </>;
    }}</Field>
    <Field name="tools">{field => {
      const tools = (field.state.value as AgentProfileDocument['tools']) ?? {};
      return <>
        <CheckboxList label={tx('settings:extension-detail.native-tools')} values={nativeTools} selected={tools.builtin ?? []}
          change={builtin => field.handleChange({ ...tools, builtin } as never)} />
        <SourceSelections value={tools.sources ?? {}} change={sources => field.handleChange({ ...tools, sources } as never)} />
      </>;
    }}</Field>
    <Field name="agents">{field => <Names label={tx('settings:extension-detail.delegated-agents')} value={(field.state.value as string[] | undefined) ?? []} change={value => field.handleChange(value as never)} />}</Field>
    <Field name="workflows">{field => <Names label={tx('settings:extension-detail.delegated-workflows')} value={(field.state.value as string[] | undefined) ?? []} change={value => field.handleChange(value as never)} />}</Field>
    <Field name="skills">{field => <Selection label={tx('settings:extension-detail.skill-prompt-visibility')} value={(field.state.value as AgentSkillSelection | undefined) ?? []} change={value => field.handleChange(value as never)} />}</Field>
    <Advanced title={tx('settings:extension-detail.extensions-and-guidance')}>
      <Field name="plugins">{field => {
        const plugins = (field.state.value as AgentProfileDocument['plugins']) ?? {};
        return <fieldset><legend>{tx('settings:extension-detail.extensions-default-off')}</legend>
          {(['todo', 'goal'] as const).map(id => <label key={id}>
            <input type="checkbox" checked={plugins[id]?.enabled ?? false}
              onChange={event => field.handleChange({ ...plugins, [id]: { enabled: event.target.checked } } as never)} />{id}
          </label>)}
          <label><input type="checkbox" checked={plugins.agent_status?.enabled ?? false}
            onChange={event => field.handleChange({ ...plugins, agent_status: { ...plugins.agent_status, enabled: event.target.checked } } as never)} />{tx('settings:extension-detail.agent-status')}</label>
        </fieldset>;
      }}</Field>
      <Field name="timeout_ms">{field => <label>{tx('settings:extension-detail.child-timeout-ms')}<input type="number" min="1"
        value={(field.state.value as string | null | undefined) ?? ''}
        onChange={event => field.handleChange((event.target.value || null) as never)} /></label>}</Field>
      <Field name="agents_md">{field => {
        const guidance = (field.state.value as AgentProfileDocument['agents_md']) ?? {};
        return <>
          <label><input type="checkbox" checked={guidance.inherit ?? true} onChange={event => field.handleChange({ ...guidance, inherit: event.target.checked } as never)} />{tx('settings:agent-page.include-project-guidance')}</label>
          <Names label={tx('settings:extension-detail.project-guidance-files')} value={guidance.files ?? []} change={files => field.handleChange({ ...guidance, files } as never)} />
        </>;
      }}</Field>
    </Advanced>
  </>;
}

function SourceSelections({ value, change }: { value: Record<string, SourceToolSelection>; change: (value: Record<string, SourceToolSelection>) => void }) {
  const tx = useTranslation();
  return <>
    <p>{tx('settings:extension-detail.mcp-and-managed-python-definitions-do-not-activate-tools-select')}</p>
    {Object.entries(value).map(([id, selection]) => <div key={id}>
      <Selection label={id} value={selection} change={next => change({ ...value, [id]: next })} />
      <Button onClick={() => { const next = { ...value }; delete next[id]; change(next); }}>{tx('settings:extension-detail.remove-source-selection')}{' '}{id}</Button>
    </div>)}
  </>;
}
