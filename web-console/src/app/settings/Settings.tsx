/* Copyright (c) 2026 DeepSeek. MIT. Adapted Settings shell; see PROVENANCE.md. */
import { shallowEqual, useSelector } from '@xstate/react';
import type { SourceScope } from '../../../../protocol/app-server/v18';
import type { AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { UnitForm } from './forms/bridge';
import css from '../../presentation/settings/SettingsContent.module.css';
import { SourceContext } from './source-context';
import { SettingsActorContext, useSettingsTarget } from './machines/react';
import { mutationOutcome, type MutationOutcome } from './machines/settings-target';
import { configurationSystem, type TransactionOwner } from './machines/system';
import { SettingsPanel } from '../../presentation/settings/SettingsRoot';
import type { ConnectionController } from '../../connection/controller';
import { ConnectionSettings } from './ConnectionSettings';
import type { ProductHostWorkspaces } from '../../workspaces/host';
import { GeneralPage } from './general/GeneralPage';
import { ModelsPage } from './models/ModelsPage';
import { AgentPage } from './agent/AgentPage';
import { ToolsPage } from './tools/ToolsPage';
import { ExtensionsPage } from './extensions/ExtensionsPage';
import { AdvancedPage } from './advanced/AdvancedPage';
import {
  catalogIdentities, configAuthoring, settingsLifecycle, settingsLifecycleLabel, settingsPageLabel,
  settingsPages, settingsTargetKey, settingsTargetLabel, settingsTargetScope,
  type SettingsFocus, type SettingsPage, type SettingsTarget,
} from './projection';

export interface SettingsProps {
  client: AppServerClient; target: SettingsTarget; host?: ProductHostWorkspaces; onClose?: () => void;
  theme?: 'light' | 'dark'; setTheme?: (theme: 'light' | 'dark') => void;
  connection?: ConnectionController;
  /** The displayed product page and the detail focused inside it, both owned by
   * the Settings navigation machine. This component holds no navigation state. */
  page: SettingsPage; focus?: SettingsFocus;
  onSelect: (page: SettingsPage) => void;
  onFocus: (focus?: SettingsFocus) => void;
}

/** The presentation of one mutation outcome that is not attributable to a
 * single unit — a read or convergence fact of the target as a whole. Each
 * unit's own save reports itself, next to the unit it is about. */
function MutationNotice({ outcome }: { outcome: MutationOutcome }) {
  switch (outcome.kind) {
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

/** The Settings presentation of one exact configuration owner, organized
 * around six product pages.
 *
 * Product organization is presentation and nothing else. Native scope
 * authority, semantic-unit identities, exact-CAS writes, application state and
 * Session adoption are unchanged underneath: what moved is which page a control
 * appears on, not who owns the value it edits.
 *
 * This component owns no asynchronous configuration semantics at all. Target
 * lifetime, authoritative read ordering, mutation submission, definitive
 * acknowledgement, authoritative reread, per-unit CAS transactions and the
 * single level-triggered convergence obligation all belong to the Settings
 * authority actor this presentation attaches to. Opening, closing, changing
 * page, focusing a detail and switching target are presentation events; none of
 * them cancels a native commit or discards an editing transaction. */
export function Settings({ client, target, host, onClose = () => {}, theme = 'light', setTheme, connection, page, focus, onSelect, onFocus }: SettingsProps) {
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
  // decided once for every page whose editors author it. A malformed document
  // admits exactly one mutation — `repair_config` — and no editor may advertise
  // any other, while the independent resource documents keep their own
  // authority.
  const config = configAuthoring(source, scope);
  // One identity-discovery rule for every model selector, shared with the
  // Models page: this scope's authored identities, plus the native effective
  // ones for a Workspace when resolution produced them.
  const models = catalogIdentities(source, scope, 'models');
  const lifecycle = settingsLifecycle({ connection: transport.connection, hasSource: !!observed, targetValid, readError });
  const pages = settingsPages(target);
  // The navigation machine owns which page is shown; a page this owner does
  // not authorize can never be selected, so the landing page stands in only if
  // a target change and a render race ever disagreed.
  const current = pages.includes(page) ? page : pages[0];
  // A page change remounts the editor subtree so its local picker state does
  // not leak across pages. Editing transactions are deliberately not part of
  // that subtree, and the key deliberately carries no source revision, so
  // neither a page change nor an authoritative read can remount a dirty form.
  const editorKey = `${transport.endpoint ?? ''}|${transport.authorityRevision ?? 0}|${settingsTargetKey(target)}:${current}`;
  const connectionFocused = current === 'advanced' && focus?.kind === 'connection' && !!connection;

  // Authoring stays closed until the target holds a current authoritative
  // observation. A mutation in flight does not close it: every other unit
  // stays editable, and whether any unit may submit is the actor's one
  // target-wide admission fact, which each unit form reads.
  const editable = !!selected && !!observed && targetValid && transport.connection === 'connected';
  const structured = config.state === 'structured';
  const body = !source || !selected ? null : <fieldset disabled={!editable} className={css.editor}>
    {current === 'models' && (structured
      ? <ModelsPage source={source} scope={scope} revision={config.revision} models={models} focus={focus} onFocus={onFocus} />
      : <MalformedNotice config={config} />)}
    {current === 'agent' && (structured
      ? <AgentPage document={config.document} scope={scope} revision={config.revision} />
      : <MalformedNotice config={config} />)}
    {current === 'tools' && (structured
      ? <ToolsPage source={source} document={config.document} scope={scope} revision={config.revision} />
      : <MalformedNotice config={config} />)}
    {/* Extensions spans several independent native documents. A malformed
        `rustx.toml` closes the semantic units it authors and says nothing
        about the MCP or named-Agent documents, each of which is its own
        authority and reports its own state. */}
    {current === 'extensions' && <ExtensionsPage source={source} scope={scope}
      revision={structured ? config.revision : undefined} models={models} focus={focus} onFocus={onFocus} />}
    {current === 'advanced' && <AdvancedPage source={source} scope={scope}
      config={structured ? { document: config.document, revision: config.revision } : undefined}
      closed={<MalformedNotice config={config} diagnostic={false} />}
      processPolicyImpacts={source.process_policy_impacts} busy={busy} targetValid={targetValid} />}
    {/* The one mutation a malformed `rustx.toml` admits, fenced on its exact
        current revision. It exists only while the document does not parse. */}
    {config.state === 'malformed' && current !== 'extensions' && <UnitForm title="Repair malformed source" blank="" revision={config.revision} removable={false}
      mutation={replacement => ({ kind: 'repair_config', document: replacement ?? '' })}>
      {(value, change) => <label>Replacement TOML<textarea value={value} onChange={event => change(event.target.value)} /></label>}
    </UnitForm>}
  </fieldset>;

  return <SettingsPanel pages={pages.map(id => ({ id, label: settingsPageLabel(id) }))} activeId={current}
    onSelect={id => onSelect(id as SettingsPage)} onClose={onClose}>
    {/* The owner this dialog is bound to, and the state of its authoritative
        observation, identify every page alike — including the client-owned
        General page, which authors no native source but still belongs to one
        Settings instance. The native source path, its revision and the raw
        projections stay on Advanced. */}
    <section className={css.settings} aria-label="Settings" aria-busy={busy}>
      <h2>{settingsTargetLabel(target)}</h2>
      {scope === 'workspace' && <p className={css.hint}>Bound to this exact authorized Workspace. Session focus never retargets this editor.</p>}
      <p role="status" data-lifecycle={lifecycle}>{settingsLifecycleLabel(lifecycle)}</p>
      {/* An authoritative read of this target, available on every page and
          independent of whether a projection is currently held: a target with
          no current observation is exactly the state that needs it most. It is
          a read, never a rescan — rediscovering configuration files is the
          separate native maintenance operation on Advanced. */}
      <Button disabled={busy || transport.connection !== 'connected'} onClick={() => actor.send({ type: 'REFRESH' })}>Reload configuration</Button>
      {readError && <p role="alert">Source read failed. {readError}</p>}
      {convergenceError && <p role="alert">{convergenceError}</p>}
      <MutationNotice outcome={outcome} />
      {maintenanceError && <p role="alert">{maintenanceError}</p>}
      {scope === 'workspace' && current !== 'general' && <p className={css.hint}>Use global default removes the unit this Workspace authors, so the global value applies again. It never removes the global definition.</p>}
      {current === 'general' ? <GeneralPage theme={theme} setTheme={setTheme} /> : connectionFocused
        ? <ConnectionSettings connection={connection!} client={client} />
        : <SettingsActorContext value={actor}><SourceContext value={source}>
          <div key={editorKey}>{body}</div>
        </SourceContext></SettingsActorContext>}
      {current === 'advanced' && connection && <p>
        <Button onClick={() => onFocus(connectionFocused ? undefined : { kind: 'connection' })}>
          {connectionFocused ? 'Back to Advanced' : 'Connection'}
        </Button>
      </p>}
    </section>
  </SettingsPanel>;
}

/** A document that does not parse is named with the native reason it failed,
 * because that reason is what the repair below has to address. Advanced
 * reports that reason with its other native diagnostics instead. */
function MalformedNotice({ config, diagnostic = true }: {
  config: { state: 'malformed'; path: string; diagnostic: string } | { state: 'structured' } | { state: 'unavailable' };
  diagnostic?: boolean;
}) {
  if (config.state !== 'malformed') return null;
  return <>
    <p role="status">Structured editing is unavailable because {config.path} does not parse. Repair the source to edit it again.</p>
    {diagnostic && <p role="alert">{config.diagnostic}</p>}
  </>;
}
