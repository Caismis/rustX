/* Copyright (c) 2026 DeepSeek. MIT. Adapted Settings shell; see PROVENANCE.md. */
import { shallowEqual, useSelector } from '@xstate/react';
import type { SourceScope } from '../../../../protocol/app-server/v18';
import type { AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { ResourceInventory } from './ResourceInventory';
import { CatalogEditor } from './CatalogEditor';
import { AgentEditor } from './AgentEditor';
import { Integrations } from './Integrations';
import { RootEditor, type RootSection } from './RootEditor';
import { RuntimeEditor } from './RuntimeEditor';
import { UnitForm } from './controls';
import css from '../../presentation/settings/SettingsContent.module.css';
import { SourceContext } from './source-context';
import { SettingsActorContext, useSettingsTarget } from './machines/react';
import { mutationOutcome, type MutationOutcome } from './machines/settings-target';
import type { SettingsSection } from './machines/navigation';
import { configurationSystem, type TransactionOwner } from './machines/system';
import { SettingsPanel } from '../../presentation/settings/SettingsRoot';
import type { ConnectionController } from '../../connection/controller';
import { ConnectionSettings } from './ConnectionSettings';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import {
  catalogIdentities, changeBehavior, changeBehaviorLabel, configAuthoring, observedResult, observedResultLabel, observedUnitLabel,
  observedUnits, settingsLifecycle, settingsLifecycleLabel, settingsTargetKey,
  settingsTargetLabel, settingsTargetScope, unitApplication, type SettingsTarget,
} from './projection';

/** The native document a section's editors mutate.
 *
 * - `config`: the scope's `rustx.toml`, through semantic-unit mutations;
 * - `resources`: documents independent of `rustx.toml` — MCP, named Agent
 *   resources and resource inventories — each governed by its own native state;
 * - `none`: nothing is authored here.
 *
 * This is what lets the composition decide once, for a whole section, whether
 * its structured editors can exist: no individual editor rediscovers it. */
type SectionDocument = 'config' | 'resources' | 'none';
const sections: readonly (readonly [SettingsSection, string, string, SectionDocument])[] = [
  ['overview', 'Overview', 'General', 'none'], ['general', 'General', 'General', 'config'], ['catalog', 'Providers & Models', 'Models', 'config'],
  ['root-model', 'Default model', 'Models', 'config'], ['policies', 'Tool Policies', 'Agents & Tools', 'config'],
  ['root-tools', 'Tools', 'Agents & Tools', 'config'], ['root-skills', 'Skill access', 'Agents & Tools', 'config'],
  ['root-plugins', 'Plugins', 'Agents & Tools', 'config'], ['root-agents', 'Agents & Workflows', 'Agents & Tools', 'config'],
  ['agents', 'Agents', 'Agents & Tools', 'resources'], ['mcp', 'MCP', 'Integrations', 'resources'], ['python', 'Managed Python', 'Integrations', 'resources'],
  ['skills', 'Skills', 'Integrations', 'resources'], ['workflows', 'Workflows', 'Integrations', 'resources'], ['advanced', 'Server & source diagnostics', 'Advanced', 'none'],
];
const sectionDocument = (section: SettingsSection): SectionDocument => sections.find(([id]) => id === section)?.[3] ?? 'none';

export interface SettingsProps {
  client: AppServerClient; target: SettingsTarget; host?: ProductHostWorkspaces; onClose?: () => void;
  theme?: 'light' | 'dark'; setTheme?: (theme: 'light' | 'dark') => void;
  connection?: ConnectionController;
  /** The displayed surface and the selection intent, both owned by the
   * Settings navigation machine. This component holds no section state. */
  section: SettingsSection; onSelect: (section: SettingsSection) => void;
}

/** The presentation of one mutation outcome. */
function MutationNotice({ outcome }: { outcome: MutationOutcome }) {
  switch (outcome.kind) {
    case 'saved': return <p role="status">Source saved. Native coordination owns application.</p>;
    case 'conflict': return <p role="alert">Source changed. Your draft and base revision are preserved.</p>;
    case 'rejected': return <p role="alert">{outcome.detail}</p>;
    case 'uncertain': return <p role="alert">Save outcome uncertain. Authority is reread; the write is never replayed. Review the current source before saving again.</p>;
    default: return null;
  }
}

/** Test-only inspection of the live transaction owners of one client, for the
 * regression that proves a confirmed commit leaves no secret-bearing authored
 * payload reachable. Production code never reads it. */
export function settingsTransactionOwners(client: AppServerClient): readonly TransactionOwner[] {
  return configurationSystem(client).transactionOwners();
}

/** The Settings presentation of one exact configuration owner.
 *
 * This component owns no asynchronous configuration semantics at all. Target
 * lifetime, authoritative read ordering, mutation submission, definitive
 * acknowledgement, authoritative reread, per-unit CAS transactions and the
 * single level-triggered convergence obligation all belong to the Settings
 * authority actor this presentation attaches to. Opening, closing, changing
 * section and switching target are presentation events; none of them cancels a
 * native commit or discards an editing transaction. */
export function Settings({ client, target, host, onClose = () => {}, theme = 'light', setTheme, connection, section, onSelect }: SettingsProps) {
  const { actor, transport } = useSettingsTarget(client, target, host);
  const scope: SourceScope = settingsTargetScope(target);
  // The fresh authoritative observation, and the last one demoted to stale
  // presentation data by a presentation or generation boundary. Rendering the
  // stale value keeps the presentation continuous across a dialog reopen; it is
  // never treated as current — the machine owes a fresh read on every ATTACH,
  // and authoring stays closed until that read is adopted.
  const observed = useSelector(actor, snapshot => snapshot.context.observation);
  const source = useSelector(actor, snapshot => snapshot.context.observation ?? snapshot.context.staleObservation);
  const readError = useSelector(actor, snapshot => snapshot.context.readError);
  const convergenceError = useSelector(actor, snapshot => snapshot.context.convergenceError);
  const maintenanceError = useSelector(actor, snapshot => snapshot.context.maintenanceError);
  const outcome = useSelector(actor, mutationOutcome, shallowEqual);
  const busy = useSelector(actor, snapshot => snapshot.matches({ mutation: 'submitting' }));
  // A successful authoritative read clears only the read error it answers, so
  // "this target is observable" is exactly "no read failure is outstanding".
  const targetValid = !readError;
  const selected = source?.[scope];
  // Whether this scope's `rustx.toml` admits structured semantic-unit editing,
  // decided once for every config-backed section. A malformed document admits
  // exactly one mutation — `repair_config` — and no editor may advertise any
  // other, while the independent resource documents keep their own authority.
  const config = configAuthoring(source, scope);
  const document = sectionDocument(section);
  // One identity-discovery rule for every model selector, shared with the
  // Providers & Models catalog: this scope's authored identities, plus the
  // native effective ones for a Workspace when resolution produced them.
  const models = catalogIdentities(source, scope, 'models');
  const roots = [source?.user_resource_root ? source.user_resource_root + '/skills' : '', source?.workspace_resource_root ? source.workspace_resource_root + '/skills' : ''];
  const lifecycle = settingsLifecycle({ connection: transport.connection, hasSource: !!observed, targetValid, readError });
  // A section change remounts the editor subtree so its local picker state does
  // not leak across sections. Editing transactions are deliberately not part of
  // that subtree, so they survive the remount.
  const editorKey = `${transport.endpoint ?? ''}|${transport.authorityRevision ?? 0}|${settingsTargetKey(target)}:${section}`;
  // Authoring stays closed until the target holds a current authoritative
  // observation. A mutation in flight does not close it: every other unit
  // stays editable, and whether any unit may submit is the actor's one
  // target-wide admission fact, which each unit form reads.
  const editor = selected && <fieldset disabled={!targetValid || !observed || transport.connection !== 'connected'} className={css.editor}>
    {document === 'config' && config.state === 'structured' && <>
      {section === 'catalog' && <CatalogEditor source={source!} scope={scope} revision={config.revision} />}
      {(section === 'general' || section === 'policies') && <RuntimeEditor document={config.document} resolved={source!.resolved} scope={scope} revision={config.revision} policyOnly={section === 'policies'} processPolicyImpacts={source!.process_policy_impacts} />}
      {section.startsWith('root-') && <RootEditor document={config.document} resolved={source!.resolved} scope={scope} revision={config.revision} section={section as RootSection} models={models} skillRoots={roots} />}
    </>}
    {document === 'config' && config.state === 'malformed' && <p role="status">Structured editing is unavailable because {config.path} does not parse. Repair the source to edit it again.</p>}
    {section === 'mcp' && <Integrations source={source!} scope={scope} />}
    {section === 'agents' && <AgentEditor source={source!} scope={scope} models={models} />}
    {document === 'resources' && source?.prospective_resources && <ResourceInventory resources={source.prospective_resources} family={section} scope={scope} />}
    {/* The one mutation a malformed `rustx.toml` admits, fenced on its exact
        current revision. It exists only while the document does not parse, and
        never in a section that edits an independent document. */}
    {config.state === 'malformed' && document !== 'resources' && <UnitForm title="Repair malformed source" blank="" revision={config.revision} removable={false}
      mutation={replacement => ({ kind: 'repair_config', document: replacement ?? '' })}>
      {(value, change) => <label>Replacement TOML<textarea value={value} onChange={event => change(event.target.value)} /></label>}
    </UnitForm>}
  </fieldset>;
  return <SettingsPanel rows={[{ id: 'appearance', label: 'Appearance' }, ...(connection ? [{ id: 'connection', label: 'Connection' }] : []), ...sections.map(([id, label, group]) => ({ id, label, group }))]} activeId={section} onSelect={id => onSelect(id as SettingsSection)} onClose={onClose}>
    {section === 'connection' && connection ? <ConnectionSettings connection={connection} client={client} /> : section === 'appearance' ?
      <section><h2>Appearance</h2><label>Theme<select aria-label="Theme" value={theme} onChange={event => setTheme?.(event.target.value as 'light' | 'dark')}><option value="light">Light</option><option value="dark">Dark</option></select></label></section> :
      <section className={css.settings} aria-label="Settings" aria-busy={busy}>
        <h2>{settingsTargetLabel(target)}</h2>
        {scope === 'workspace' && <p className={css.hint}>Bound to this exact authorized Workspace. Session focus never retargets this editor.</p>}
        <p role="status" data-lifecycle={lifecycle}>{settingsLifecycleLabel(lifecycle)}</p>
        <Button disabled={busy || transport.connection !== 'connected'} onClick={() => actor.send({ type: 'REFRESH' })}>Read current sources</Button>
        {readError && <p role="alert">Source read failed. {readError}</p>}
        {convergenceError && <p role="alert">{convergenceError}</p>}
        <MutationNotice outcome={outcome} />
        {maintenanceError && <p role="alert">{maintenanceError}</p>}
        {selected && <p>{selected.path} · Revision: {selected.revision}</p>}
        {selected?.diagnostic && <p role="alert">{selected.diagnostic}</p>}
        {source?.prospective_diagnostic && <p role="status">{source.prospective_diagnostic}</p>}
        {scope === 'workspace' && <p>Remove an override to reset to the global default. An explicit empty selection means none.</p>}
        <h3>{sections.find(([id]) => id === section)?.[1]}</h3>
        <SettingsActorContext value={actor}><SourceContext value={source}><div key={editorKey}>{editor}</div></SourceContext></SettingsActorContext>
        {section === 'overview' && <p>Definitions and defaults belong to this source. Session selections and explicit adoption belong to each Session.</p>}
        {section === 'advanced' && source && <>
          <h3>Application observation</h3>
          <ul>{observedUnits.map(unit => {
            const result = observedResult(unitApplication(source.application, unit));
            return <li key={unit}>{observedUnitLabel(unit)}: <strong>{observedResultLabel(result)}</strong>
              {result.state === 'failed' && <> — {result.diagnostic}</>}
              {result.state === 'ready' && <> — cache impact {result.impact}</>}
            </li>;
          })}</ul>
          <p>Applied, Preparing, Failed and Restart pending are native observations of this exact source scope. They are never Session adoption and never one global success state.</p>
          <h3>Change behavior</h3>
          <ul>{Object.keys(source.process_policy_impacts).map(key => <li key={key}>{key}: {changeBehaviorLabel(changeBehavior(source.process_policy_impacts, key))}</li>)}</ul>
          <h3>Process bindings</h3><pre>{JSON.stringify(source.process_bindings, null, 2)}</pre>
          {source.application?.units.process_bindings?.status === 'process_restart' && <p role="status">Saved desired values differ from the current process binding. Restart required.</p>}
          {source.application?.units.process_bindings?.status === 'applied' && <p role="status">Saved process policy is active.</p>}
          {/* Both diagnostics render the native projection verbatim. That is
              safe because the projection itself is redacted: Provider
              credentials, MCP literal `env`/`headers` and literal Tool
              environment values are identity-only on the wire, so there is no
              secret here to hide. */}
          <details><summary>Resolved preview — source resolution only</summary><pre>{JSON.stringify({ resolved: source.resolved, provenance: source.provenance }, null, 2)}</pre></details>
          <details><summary>Source and application diagnostics</summary><pre>{JSON.stringify(source, null, 2)}</pre></details>
          <Button disabled={busy || !targetValid} onClick={() => actor.send({ type: 'RECONCILE' })}>Rescan configuration files</Button>
        </>}
      </section>}
  </SettingsPanel>;
}
