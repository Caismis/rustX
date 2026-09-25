/* Copyright (c) 2026 DeepSeek. MIT. Adapted Settings shell; see PROVENANCE.md. */
import type { ReactNode } from 'react';
import { shallowEqual, useSelector } from '@xstate/react';
import type { SourceScope } from '../../../../protocol/app-server/v23';
import type { AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { UnitForm } from './forms/bridge';
import css from '../../presentation/settings/SettingsContent.module.css';
import { SourceContext } from './source-context';
import { SettingsActorContext, useSettingsTarget } from './machines/react';
import { mutationOutcome, type MutationOutcome } from './machines/settings-target';
import { configurationSystem, type TransactionOwner } from './machines/system';
import { SettingsPanel } from '../../presentation/settings/SettingsRoot';
import {
  IconAgentPresetOutline16, IconCodeOutline16, IconDataOutline16, IconPersonalizationOutline16,
  IconPluginPinwheelOutline16, IconShieldOutline16,
} from '../../presentation/primitives/icons';
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
  catalogIdentities, configAuthoring, settingsLifecycle, settingsLifecycleLabel,
  settingsTargetKey, settingsTargetLabel, settingsTargetScope, type SettingsTarget,
} from './projection';
import {
  admitsFocus, connectionFocus, settingsPageLabel, settingsPages,
  type FocusMap, type SettingsFocus, type SettingsNavigationActor, type SettingsPage,
} from './machines/navigation';

export interface SettingsProps {
  client: AppServerClient; host?: ProductHostWorkspaces;
  theme?: 'light' | 'dark'; setTheme?: (theme: 'light' | 'dark') => void;
  /** The client-owned connection, rendered only where navigation admits the
   * Connection surface. Its presence authorizes nothing. */
  connection: ConnectionController;
  /** The one owner of whether Settings is open, which target it is bound to,
   * which page it shows and which detail is focused inside that page. This
   * component holds no navigation state and renders that state as it is. */
  navigation: SettingsNavigationActor;
}

/** The presentation of one mutation outcome that is not attributable to a
 * single unit — a read or convergence fact of the target as a whole. Each
 * unit's own save reports itself, next to the unit it is about. */
function MutationNotice({ outcome }: { outcome: MutationOutcome }) {
  switch (outcome.kind) {
    case 'uncertain': return <p role="alert" className={css.error}>Save outcome uncertain. Authority is reread; the write is never replayed. Review the current source before saving again.</p>;
    default: return null;
  }
}

/** Each primary page's glyph from the existing Harness icon family. They tell
 * the pages apart at a glance; the page label stays the accessible name, so
 * nothing depends on recognizing a glyph or a color. */
const pageIcons: Record<SettingsPage, () => ReactNode> = {
  general: () => <IconPersonalizationOutline16 />,
  models: () => <IconDataOutline16 />,
  agent: () => <IconAgentPresetOutline16 />,
  tools: () => <IconShieldOutline16 />,
  extensions: () => <IconPluginPinwheelOutline16 />,
  advanced: () => <IconCodeOutline16 />,
};

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
export function Settings(props: SettingsProps) {
  const view = useSelector(props.navigation, snapshot => ({
    target: snapshot.context.target, page: snapshot.context.page, focus: snapshot.context.focus,
  }), shallowEqual);
  return view.page && <SettingsDialog {...props} target={view.target} page={view.page} focus={view.focus} />;
}

function SettingsDialog({ client, host, theme = 'light', setTheme, connection, navigation, target, page: current, focus }: SettingsProps & {
  target: SettingsTarget; page: SettingsPage; focus: FocusMap;
}) {
  // The navigation machine admits only pages and details this owner
  // authorizes, so the page and focus are rendered exactly as they are.
  const { actor, transport } = useSettingsTarget(client, target, host);
  const onFocus = (next?: SettingsFocus) => navigation.send({ type: 'FOCUS', focus: next });
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
  // A page change remounts the editor subtree so its local picker state does
  // not leak across pages. Editing transactions are deliberately not part of
  // that subtree, and the key deliberately carries no source revision, so
  // neither a page change nor an authoritative read can remount a dirty form.
  const editorKey = `${transport.endpoint ?? ''}|${transport.authorityRevision ?? 0}|${settingsTargetKey(target)}:${current}`;
  const connectionFocused = current === 'advanced' && focus.advanced !== undefined;
  // Whether this owner's Advanced page reaches Connection at all is the
  // navigation capability, not whether a connection controller exists.
  const connectionReachable = current === 'advanced' && admitsFocus(target, 'advanced', connectionFocus);

  // Authoring stays closed until the target holds a current authoritative
  // observation. A mutation in flight does not close it: every other unit
  // stays editable, and whether any unit may submit is the actor's one
  // target-wide admission fact, which each unit form reads.
  const editable = !!selected && !!observed && targetValid && transport.connection === 'connected';
  const structured = config.state === 'structured';
  const body = !source || !selected ? null : <fieldset disabled={!editable} className={css.editor}>
    {current === 'models' && (structured
      ? <ModelsPage source={source} scope={scope} revision={config.revision} models={models} focus={focus.models} onFocus={onFocus} />
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
      revision={structured ? config.revision : undefined} models={models} focus={focus.extensions} onFocus={onFocus} />}
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

  const pages = settingsPages(target).map(id => ({ id, label: settingsPageLabel(id), icon: pageIcons[id]() }));
  return <SettingsPanel pages={pages} activeId={current}
    onSelect={id => navigation.send({ type: 'SELECT', page: id as SettingsPage })} onClose={() => navigation.send({ type: 'CLOSE' })}
    // The owner this dialog is bound to, and the state of its authoritative
    // observation, identify every page alike — including the client-owned
    // General page, which authors no native source but still belongs to one
    // Settings instance. They sit in the fixed header, with the authoritative
    // reread of this target: a read, never a rescan — rediscovering
    // configuration files is the separate native maintenance operation on
    // Advanced. The native source path, its revision and the raw projections
    // stay on Advanced.
    context={<div className={css.context}>
      <div className={css.contextTitle}>
        <h2>{settingsTargetLabel(target)}</h2>
        <p role="status" className={css.lifecycle} data-lifecycle={lifecycle}>{settingsLifecycleLabel(lifecycle)}</p>
      </div>
      <Button size="sm" variant="outline" disabled={busy || transport.connection !== 'connected'} onClick={() => actor.send({ type: 'REFRESH' })}>Reload configuration</Button>
    </div>}>
    <section className={`${css.settings} ${css.page}`} aria-label="Settings" aria-busy={busy}>
      {scope === 'workspace' && current !== 'models' && <p className={css.hint}>Bound to this exact authorized Workspace. Session focus never retargets this editor.</p>}
      {/* Target-wide facts, each reported as itself: a read failure, a
          convergence report, an unknown save outcome and a maintenance
          failure are separate alerts, and none of them hides another. */}
      {readError && <p role="alert" className={css.error}>Source read failed. {readError}</p>}
      {convergenceError && <p role="alert" className={css.error}>{convergenceError}</p>}
      <MutationNotice outcome={outcome} />
      {maintenanceError && <p role="alert" className={css.error}>{maintenanceError}</p>}
      {scope === 'workspace' && current !== 'general' && current !== 'models' && <p className={css.hint}>Use global default removes the unit this Workspace authors, so the global value applies again. It never removes the global definition.</p>}
      {current === 'general' ? <GeneralPage theme={theme} setTheme={setTheme} /> : connectionFocused
        ? <ConnectionSettings connection={connection} client={client} />
        : !source ? <SourcePending lifecycle={lifecycle} />
          : <SettingsActorContext value={actor}><SourceContext value={source}>
            <div key={editorKey}>{body}</div>
          </SourceContext></SettingsActorContext>}
      {connectionReachable && <p>
        <Button onClick={() => onFocus(connectionFocused ? undefined : connectionFocus)}>
          {connectionFocused ? 'Back to Advanced' : 'Connection'}
        </Button>
      </p>}
    </section>
  </SettingsPanel>;
}

/** The page body before this target has any projection to present — not even
 * a stale one. Loading, connecting and unavailable are different states, and
 * none of them is shown as an empty configuration. */
function SourcePending({ lifecycle }: { lifecycle: ReturnType<typeof settingsLifecycle> }) {
  const text = lifecycle === 'failed' ? 'No configuration can be shown until the source is read. Reload configuration to try again.'
    : lifecycle === 'connecting' ? 'Configuration is shown once the App Server connection is established.'
      : 'Reading this source from the App Server. Nothing is shown until native answers.';
  return <div className={css.pending} data-lifecycle={lifecycle} aria-busy={lifecycle === 'loading' || lifecycle === 'connecting'}><p>{text}</p></div>;
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
    {diagnostic && <p role="alert" className={css.error}>{config.diagnostic}</p>}
  </>;
}
