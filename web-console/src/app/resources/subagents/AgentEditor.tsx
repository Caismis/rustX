/* Copyright (c) 2026 DeepSeek. MIT. Adapted resource controls; see PROVENANCE.md. */
import { useEffect, useId, useRef } from 'react';
import clsx from 'clsx';
import type { NamedAgentProfileDocument, SourceSettings } from '../../../../../protocol/app-server/v80';
import type { AppServerClient } from '../../../client/app-server';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { Switch } from '../../../presentation/primitives/Switch';
import { SettingsBackButton, SettingsPageShell } from '../../../presentation/settings/SettingsPageShell';
import { settingsContentCss } from '../../../presentation/settings/settingsContentCss';
import { DefinitionNotice, PageTransactionFooter, UnitOutcomeNotice, UnitReset, usePageTransaction, useUnitEditing } from '../../settings/forms/bridge';
import { useNavigationGuard } from '../../settings/forms/guard';
import { Choice, ConfirmAction } from '../../settings/primitives/aria';
import { AgentModelOptions, ModelProfileChoice, type ModelProfiles } from '../../settings/models/ModelsPage';
import { SettingsNotice } from '../../settings/SettingsFeedback';
import { SettingsScopeMenu } from '../../settings/SettingsScopeMenu';
import type { SettingsTarget } from '../../settings/projection';
import type { ProductHostWorkspaces } from '../../../workspaces/host';
import { extensionEntries } from '../shared/inventory';
import { subagentsCss } from './subagentsCss';
import { AgentToolFields, ManagedToolSelections, type LoadMcpTools } from './AgentToolFields';
import { AgentSkillFields } from './AgentSkillFields';
import { Input as TextInput } from '@/presentation/primitives/Input';
import { Textarea } from '@/components/ui/textarea';

export type AgentDraft = NamedAgentProfileDocument & { name?: string };

interface Props {
  source: SourceSettings; target: SettingsTarget; host?: ProductHostWorkspaces; name?: string;
  models: string[]; profiles: ModelProfiles; initialDraft?: AgentDraft;
  client?: AppServerClient; discover?: boolean; editable?: boolean;
  loadMcpTools?: LoadMcpTools;
  onScopeChange?: (target: SettingsTarget, draft: AgentDraft) => void;
  onClose: () => void;
}

export function AgentEditor({
  source, target, host, name, models, profiles, initialDraft, client,
  discover = true, editable = true, loadMcpTools, onScopeChange, onClose,
}: Props) {
  const tx = useTranslation();
  const scope = target.kind;
  const creating = name === undefined;
  const current = source.agents.find(agent => agent.scope === scope && agent.name === name);
  const unit = useUnitEditing<AgentDraft>({
    onSaved: onClose,
    draftKey: creating ? 'create-subagent' : undefined,
    authoredPresent: current !== undefined,
    authored: current?.source.authored ?? undefined,
    blank: initialDraft ?? {},
    revision: current?.source.revision ?? source.absent_resource_revision,
    mutation: value => {
      const { name: draftName, ...authored } = value ?? {};
      return { kind: 'agent', name: name ?? draftName?.trim() ?? '', authored: value === null ? null : authored };
    },
  });
  const form = useRef<HTMLFormElement>(null);

  const page = usePageTransaction();
  const setLocal = page?.setLocal;
  const guard = useNavigationGuard();
  // No silent discard: closing through the breadcrumb asks first when the
  // page transaction reports a draft; abandoning there discards it.
  const close = () => guard.guard(onClose);
  const value = unit.displayed;
  const identity = name ?? value.name?.trim() ?? '';
  const validationId = useId();
  const promptId = useId();
  const sections = useId();
  const invalid = creating && !/^[a-z][a-z0-9_-]{0,63}$/.test(identity);
  const duplicate = creating && !!identity && (
    source.agents.some(agent => agent.name === identity) ||
    extensionEntries(source, scope, 'agent').some(entry => entry.name === identity)
  );
  const nameError = duplicate ? tx('resources:subagents.duplicate')
    : invalid && value.name ? tx('resources:subagents.name-help') : undefined;
  const busy = unit.busy || unit.awaitingObservation;
  const restoring = scope === 'workspace' && !!unit.shadowed;
  const title = tx(creating ? 'resources:subagents.new' : 'resources:subagents.edit');
  const description = tx(creating ? 'resources:subagents.create-help' : 'resources:subagents.edit-help');
  const formName = creating ? title : tx('settings:extension-detail.agent-value', { p0: name ?? '' });
  const canSubmit = editable && unit.admitted && !busy && !unit.reviewNeeded;
  const saveable = canSubmit && !invalid && !duplicate;
  const discardRef = useRef(unit.discard);
  discardRef.current = unit.discard;
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  useEffect(() => setLocal?.(
    false,
    undefined,
    creating ? () => { discardRef.current(); closeRef.current(); } : undefined,
    () => saveable,
  ), [setLocal, creating, saveable]);

  const model = value.model ?? undefined;
  const modelChoices = [...new Set([...models, ...(model?.model ? [model.model] : [])])];
  const update = (changes: Partial<AgentDraft>) => unit.edit({ ...value, ...changes });
  const guidance = value.agents_md ?? {};
  const resourceScope = unit.definition === 'inherited' ? 'user' : scope;

  const footerLeading = (
    <>
      {unit.authoredPresent && (
        <span data-removal={restoring ? 'override-removal' : 'authored-removal'}>
          <ConfirmAction
            label={tx(restoring ? 'settings:bridge.use-global-default-value' : 'settings:bridge.remove-value', { p0: formName })}
            title={tx(restoring ? 'settings:bridge.use-the-global-default-for-value' : 'settings:bridge.remove-value-from-value-configuration', {
              p0: formName, p1: tx(scope === 'workspace' ? 'settings:extension-detail.workspace' : 'settings:extension-detail.user'),
            })}
            triggerText={tx(restoring ? 'settings:models.restore' : 'settings:copy.delete')}
            confirm={tx(restoring ? 'settings:bridge.use-global-default-value' : 'settings:bridge.remove-value', { p0: formName })}
            description={tx(restoring ? 'settings:forms.restore-help' : 'resources:subagents.delete-help')}
            settle={form}
            tone={restoring ? 'restore' : 'destructive'}
            disabled={!canSubmit}
            onConfirm={() => { if (canSubmit) unit.submit(true); }}
          />
        </span>
      )}
      {unit.reviewNeeded && (
        <Button size="sm" disabled={!editable || busy} onClick={unit.review}>
          {tx('settings:bridge.use-reviewed-revision')}
        </Button>
      )}
      {unit.override && (
        <Button
          type="button"
          variant="primary"
          size="sm"
          aria-label={`${tx('settings:bridge.override')} ${formName} ${tx('settings:bridge.in-this-workspace')}`}
          disabled={!editable || busy}
          onClick={unit.override}
        >
          {tx('settings:forms.customize')}
        </Button>
      )}
    </>
  );

  const scopeControl = (
    <div className={subagentsCss.scope}>
      <span>{tx('settings:catalog.scope')}</span>
      {creating && onScopeChange ? (
        <SettingsScopeMenu
          target={target}
          host={host}
          userLabel={tx('resources:scope.global')}
          onSelect={next => { unit.discard(); onScopeChange(next, value); }}
        />
      ) : (
        <span className={subagentsCss.scopeValue}>
          {unit.definition === 'inherited' || target.kind === 'user' ? tx('resources:scope.global') : target.displayName}
        </span>
      )}
    </div>
  );

  return (
    <SettingsPageShell
      title={title}
      description={description}
      bodyDisabled={!editable || busy}
      breadcrumb={(
        <nav aria-label={tx('resources:subagents.navigation')}>
          <SettingsBackButton label={tx('resources:subagents.title')} current={name ?? title} onClick={close} />
        </nav>
      )}
      toolbar={scopeControl}
      footer={<PageTransactionFooter cancelAlwaysEnabled={creating} leading={footerLeading} />}
    >
      <div className={subagentsCss.editor}>
        <UnitReset.Provider value={unit.resets}>
          <form
            ref={form}
            tabIndex={-1}
            aria-label={formName}
            aria-busy={busy}
            className={clsx(settingsContentCss.settings, subagentsCss.form)}
            data-definition={unit.definition}
            data-authored-value={unit.authoredPresent ? unit.unparsed ? 'unparsed' : 'parsed' : undefined}
            onSubmit={event => event.preventDefault()}
          >
            {(unit.definition === 'inherited' || unit.definition === 'overriding' || unit.unparsed) && <DefinitionNotice unit={unit} />}
            <SettingsNotice text={current?.source.diagnostic ?? undefined} />
            <fieldset className={subagentsCss.fields} disabled={!unit.writable}>
              <section className={subagentsCss.section} aria-labelledby={`${sections}-profile`}>
                <div className={subagentsCss.sectionHead}>
                  <h2 id={`${sections}-profile`}>{tx('resources:subagents.profile')}</h2>
                </div>
                <div className={subagentsCss.identity}>
                  <div>
                    <label>
                      {tx('resources:subagents.name')}
                      <TextInput
                        value={name ?? value.name ?? ''}
                        readOnly={!creating}
                        maxLength={64}
                        spellCheck={false}
                        aria-invalid={nameError ? true : undefined}
                        aria-describedby={nameError ? validationId : undefined}
                        onChange={event => update({ name: event.target.value })}
                      />
                    </label>
                    {nameError && <span id={validationId} className={subagentsCss.validation}>{nameError}</span>}
                  </div>
                  <Choice
                    label={tx('settings:models-page.model')}
                    value={model?.model ?? ''}
                    options={[['', tx('resources:subagents.inherit-model')], ...modelChoices.map(id => [id, id] as const)]}
                    onChange={id => {
                      const { profile: _profile, ...rest } = model ?? {};
                      update({ model: id ? { ...rest, model: id } : null });
                    }}
                  />
                  {model?.model && (
                    <ModelProfileChoice
                      label={tx('settings:models-page.profile')}
                      profile={model.profile}
                      profiles={profiles[model.model] ?? []}
                      onChange={profile => {
                        const { profile: _profile, ...rest } = model;
                        update({ model: profile ? { ...rest, profile } : rest });
                      }}
                    />
                  )}
                </div>
                <label>
                  {tx('settings:extension-detail.description')}
                  <TextInput value={value.description ?? ''} onChange={event => update({ description: event.target.value })} />
                </label>
                <div className={subagentsCss.prompt}>
                  <label htmlFor={promptId}>{tx('resources:subagents.prompt')}</label>
                  <Textarea id={promptId} rows={4} value={value.instructions ?? ''} onChange={event => update({ instructions: event.target.value })} />
                </div>
                <div className={subagentsCss.settingRow}>
                  <span>{tx('resources:subagents.inject-guidance')}</span>
                  <Switch
                    label={tx('resources:subagents.inject-guidance')}
                    checked={guidance.inherit ?? true}
                    onChange={inherit => update({ agents_md: { ...guidance, inherit } })}
                  />
                </div>
              </section>
              <AgentToolFields
                source={source}
                scope={resourceScope}
                value={value}
                change={update}
                loadMcpTools={discover && unit.writable ? loadMcpTools : undefined}
              />
              <section className={subagentsCss.section} aria-labelledby={`${sections}-advanced`}>
                <h2 id={`${sections}-advanced`}>{tx('settings:agent-page.advanced-options')}</h2>
                <AgentSkillFields
                  client={client}
                  source={source}
                  scope={resourceScope}
                  active={discover}
                  value={value.skills ?? []}
                  change={skills => update({ skills })}
                />
                {model && <AgentModelOptions value={model} change={next => update({ model: next })} models={models} profiles={profiles} />}
                <ManagedToolSelections tools={value.tools} change={tools => update({ tools })} />
                <label className={subagentsCss.timeout}>
                  {tx('settings:extension-detail.child-timeout-ms')}
                  <TextInput type="number" min="1" value={value.timeout_ms ?? ''} onChange={event => update({ timeout_ms: event.target.value || null })} />
                </label>
                <fieldset className={subagentsCss.featureGroup}>
                  <legend>{tx('settings:extension-detail.extensions-default-off')}</legend>
                  {(['todo', 'agent_status'] as const).map(id => {
                    const label = tx(`resources:subagents.feature-${id}`);
                    return (
                      <div className={subagentsCss.settingRow} key={id}>
                        <span>{label}</span>
                        <Switch
                          label={label}
                          checked={value.plugins?.[id]?.enabled ?? false}
                          onChange={enabled => update({ plugins: { ...value.plugins, [id]: { ...value.plugins?.[id], enabled } } })}
                        />
                      </div>
                    );
                  })}
                </fieldset>
              </section>
            </fieldset>
            <UnitOutcomeNotice title={formName} outcome={unit.outcome} />
            <SettingsNotice text={unit.reviewNeeded && unit.outcome.kind !== 'conflict' ? tx(unit.committed && !unit.draft ? 'settings:forms.committed-changed-elsewhere' : 'settings:forms.changed-elsewhere') : undefined} />
          </form>
        </UnitReset.Provider>
      </div>
    </SettingsPageShell>
  );
}
