import { useState, type ReactNode } from 'react';
import type {
  AppServerPolicy, ContextLayer, RuntimeLayer, SourceScope, SourceSettings, SubagentsLayer,
  TimeoutLayer, ToolDeadlineLayer,
} from '../../../../../protocol/app-server/v24';
import { Button } from '../../../presentation/primitives/Button';
import { UnitForm } from '../forms/bridge';
import { TextField } from '../forms/controls';
import { Advanced, Choice } from '../primitives/aria';
import {
  changeBehavior, changeBehaviorLabel, observedResult, observedResultLabel, observedUnitLabel,
  observedUnits, reachableEnvironment, sourceView, unitApplication,
} from '../projection';
import { useSettingsActor } from '../machines/react';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The Advanced product page.
 *
 * Everything that requires knowing how rustX is implemented lives here: source
 * paths and revisions, native semantic-unit names, raw projections, process
 * bindings, application observations and the configuration rescan. The five
 * ordinary product pages are then free of protocol vocabulary, without any of
 * this observability being removed — it is moved, not deleted.
 *
 * Connection is a client-owned surface and is reached from here too, as an
 * Advanced sub-surface rather than a seventh primary page.
 *
 * The diagnostics and the rescan never depend on this scope's `rustx.toml`
 * parsing: a document that does not parse is exactly when its path, revision
 * and native diagnostic are needed. Only the semantic-unit editors close. */
export function AdvancedPage({ source, config, closed, scope, processPolicyImpacts, busy, targetValid }: {
  source: SourceSettings; scope: SourceScope;
  /** This scope's parsed `rustx.toml` and its exact revision, when it parses. */
  config?: { document: RuntimeLayer; revision: string };
  /** What stands in for the editors while it does not. */
  closed?: ReactNode;
  processPolicyImpacts: Record<string, 'hot' | 'restart'>; busy: boolean; targetValid: boolean;
}) {
  const actor = useSettingsActor();
  const view = sourceView(source, scope);
  return <section aria-label="Advanced">
    <h3>Advanced</h3>
    <p>Runtime limits, environment, process policy and the native diagnostics behind every other page.</p>
    {config
      ? <AdvancedEditors source={source} scope={scope} document={config.document} revision={config.revision} processPolicyImpacts={processPolicyImpacts} />
      : closed}

    <h4>Native diagnostics</h4>
    {view && <p>{view.path} · Revision: {view.revision}</p>}
    {view?.diagnostic && <p role="alert" className={css.error}>{view.diagnostic}</p>}
    {source.prospective_diagnostic && <p role="status">{source.prospective_diagnostic}</p>}

    <h5>Application observation</h5>
    <ul>{observedUnits.map(unit => {
      const result = observedResult(unitApplication(source.application, unit));
      return <li key={unit}>{observedUnitLabel(unit)}: <strong>{observedResultLabel(result)}</strong>
        {result.state === 'failed' && <> — {result.diagnostic}</>}
        {result.state === 'ready' && <> — cache impact {result.impact}</>}
      </li>;
    })}</ul>
    <p>Applied, Preparing, Failed and Restart pending are native observations of this exact source scope. They are never Session adoption and never one global success state.</p>

    <h5>Change behavior</h5>
    <ul>{Object.keys(source.process_policy_impacts).map(key => <li key={key}>{key}: {changeBehaviorLabel(changeBehavior(source.process_policy_impacts, key))}</li>)}</ul>

    <h5>Process bindings</h5>
    <pre>{JSON.stringify(source.process_bindings, null, 2)}</pre>
    {source.application?.units.process_bindings?.status === 'process_restart' && <p role="status">Saved desired values differ from the current process binding. Restart required.</p>}
    {source.application?.units.process_bindings?.status === 'applied' && <p role="status">Saved process policy is active.</p>}

    {/* Both diagnostics render the native projection verbatim. That is safe
        because the projection itself is redacted: Provider credentials, MCP
        literal `env`/`headers` and literal Tool environment values are
        identity-only on the wire, so there is no secret here to hide. */}
    <Advanced title="Resolved preview — source resolution only">
      <pre aria-label="Resolved preview projection">{JSON.stringify({ resolved: source.resolved, provenance: source.provenance }, null, 2)}</pre>
    </Advanced>
    <Advanced title="Source and application diagnostics">
      <pre aria-label="Source and application projection">{JSON.stringify(source, null, 2)}</pre>
    </Advanced>
    <Button disabled={busy || !targetValid} onClick={() => actor.send({ type: 'RECONCILE' })}>Rescan configuration files</Button>
  </section>;
}

/** The semantic-unit editors of this scope's parsed `rustx.toml`. */
function AdvancedEditors({ source, scope, document, revision, processPolicyImpacts }: {
  source: SourceSettings; scope: SourceScope; document: RuntimeLayer; revision: string;
  processPolicyImpacts: Record<string, 'hot' | 'restart'>;
}) {
  const [environment, setEnvironment] = useState('');
  const [selectedEnvironment, selectEnvironment] = useState('');
  const resolved = source.resolved;
  return <>
    <h4>Context and runtime limits</h4>
    <UnitForm<ContextLayer> title="Context policy" authored={document.context ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'context', authored } })}>
      {(value, change) => <>
        {(['reserve_tokens', 'keep_recent_tokens'] as const).map(key =>
          <TextField key={key} label={key} value={value[key]} change={next => change({ ...value, [key]: next || null })} />)}
        <Choice label="Summary output" value={value.summary_output_cap?.mode ?? ''}
          options={[['', 'Domain default'], ['model_limit', 'Model limit'], ['limit', 'Explicit token limit']]}
          onChange={mode => change({ ...value, summary_output_cap: mode === 'limit' ? { mode: 'limit', tokens: 2048 } : mode === 'model_limit' ? { mode: 'model_limit' } : null })} />
        {value.summary_output_cap?.mode === 'limit' && <label>Summary token limit<input type="number" min="1"
          value={value.summary_output_cap.tokens} onChange={event => change({ ...value, summary_output_cap: { mode: 'limit', tokens: Number(event.target.value) } })} /></label>}
      </>}
    </UnitForm>

    <UnitForm<TimeoutLayer> title="Model timeout" authored={document.model_timeout_policy ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'model_timeout', authored } })}>
      {(value, change) => <>{(['response_start_timeout_ms', 'stream_idle_timeout_ms'] as const).map(key =>
        <TextField key={key} label={key} value={value[key]} change={next => change({ ...value, [key]: next || null })} />)}</>}
    </UnitForm>

    <UnitForm<ToolDeadlineLayer> title="Tool deadline" authored={document.tool_deadline_policy ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'tool_deadline', authored } })}>
      {(value, change) => <>
        <TextField label="Hard deadline (ms)" value={value.hard_deadline_ms} change={hard_deadline_ms => change({ ...value, hard_deadline_ms: hard_deadline_ms || null })} />
        <Choice label="Idle liveness" value={value.idle_liveness_ms?.mode ?? ''}
          options={[['', 'Domain default'], ['disabled', 'Disabled'], ['window', 'Idle window']]}
          onChange={mode => change({ ...value, idle_liveness_ms: mode === 'window' ? { mode: 'window', milliseconds: '30000' } : mode === 'disabled' ? { mode: 'disabled' } : null })} />
        {value.idle_liveness_ms?.mode === 'window' && <TextField label="Idle window (ms)" value={value.idle_liveness_ms.milliseconds}
          change={milliseconds => change({ ...value, idle_liveness_ms: { mode: 'window', milliseconds } })} />}
      </>}
    </UnitForm>

    <UnitForm<SubagentsLayer> title="Child capacity" authored={document.subagents ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'capacity', authored } })}>
      {(value, change) => <label>Maximum concurrent children<input type="number" min="1" value={value.max_concurrent ?? ''}
        onChange={event => change({ max_concurrent: event.target.value ? Number(event.target.value) : null })} /></label>}
    </UnitForm>

    <h4>Environment</h4>
    {/* Native projects environment *identities* only: `RuntimeLayer.environment`
        is a list of names on the wire, so an inherited value cannot be
        enumerated, displayed or copied here even in principle. */}
    <p className={css.hint}>Only the names are projected to the browser. A literal Tool environment value is a secret on the same terms as a Provider credential: it is never read back, so authoring one replaces it outright.</p>
    <div className={css.actions}>
      <TextField label="Environment variable identity" value={environment} change={setEnvironment} />
      <Button disabled={!environment} onClick={() => selectEnvironment(environment)}>Edit environment variable</Button>
    </div>
    <ul>{reachableEnvironment(scope, document.environment, resolved?.environment).map(name =>
      <li key={name}><Button onClick={() => selectEnvironment(name)}>{name}</Button></li>)}</ul>
    {selectedEnvironment && <UnitForm<string> key={selectedEnvironment} title={`Environment ${selectedEnvironment}`}
      authored={document.environment?.includes(selectedEnvironment) ? '' : undefined} blank="" revision={revision} redacted
      mutation={authored => ({ kind: 'config', mutation: { unit: 'environment', name: selectedEnvironment, authored } })}>
      {(value, change) => <TextField label="Literal Tool environment value" secret value={value} change={change} />}
    </UnitForm>}

    {scope === 'user' && <>
      <h4>App Server process policy</h4>
      <p className={css.hint}>Native change behavior: {Object.keys(processPolicyImpacts).map(name => `${name}: ${changeBehaviorLabel(changeBehavior(processPolicyImpacts, name))}`).join('; ')}</p>
      <UnitForm<AppServerPolicy> title="App Server policy" authored={document.app_server ?? undefined} blank={{}} revision={revision}
        mutation={authored => ({ kind: 'config', mutation: { unit: 'app_server', authored } })}>
        {(value, change) => <>{(['max_resident_runtimes', 'max_connections', 'max_external_attachments', 'idle_grace_ms', 'shutdown_deadline_ms'] as const).map(key =>
          <label key={key}>{key}<input type="number" min="1" value={value[key] ?? ''}
            onChange={event => change({ ...value, [key]: event.target.value ? Number(event.target.value) : undefined })} /></label>)}</>}
      </UnitForm>
    </>}
    {scope === 'workspace' && <p className={css.hint}>App Server process policy is User-only. Native application state below reports which changes have applied.</p>}
  </>;
}
