import { useTranslation } from '../../../locale/react';
import { useState, type ReactNode } from 'react';
import type {
  RuntimeLayer, SourceScope, SourceSettings, SubagentsLayer,
  TimeoutLayer, ToolDeadlineLayer,
} from '../../../../../protocol/app-server/v79';
import { Button } from '../../../presentation/primitives/Button';
import { UnitForm } from '../forms/bridge';
import { TextField } from '../forms/controls';
import { Choice } from '../primitives/aria';
import {
  reachableEnvironment,
} from '../projection';
import { useSettingsActor } from '../machines/react';
import { AutoRepairSection } from './AutoRepair';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** User-adjustable runtime limits and environment values. Source revisions,
 * raw projections and process internals are not settings controls. */
export function AdvancedPage({ source, config, closed, scope, busy, targetValid, repairFocused = false, inspection, repairPresented }: {
  source: SourceSettings; scope: SourceScope;
  /** This scope's parsed `rustx.toml` and its exact revision, when it parses. */
  config?: { document: RuntimeLayer; revision: string };
  /** What stands in for the editors while it does not. */
  closed?: ReactNode;
  busy: boolean; targetValid: boolean;
  /** Opened at Auto Repair, from a repair notification's View details. */
  repairFocused?: boolean;
  /** The View details inspection awaiting these details, if any. */
  inspection?: unknown;
  /** Reports Auto Repair details presented from a current authoritative
   * read; absent while `source` is only stale presentation data. */
  repairPresented?: (receiptId: string) => void;
}) {
  const tx = useTranslation();
  const actor = useSettingsActor();
  return <section aria-label={tx('settings:advanced-page.advanced')}>
    <h3>{tx('settings:advanced-page.advanced')}</h3>
    <p>{tx('settings:advanced-page.runtime-limits-environment-process-policy-and-the-native-diagnos')}</p>
    {config
      ? <AdvancedEditors source={source} scope={scope} document={config.document} revision={config.revision} />
      : closed}

    {/* This owner's own automatic repair, from the same authoritative read.
        A receipt never touches an editor above: drafts, CAS bases and saves
        stay exactly where they were. */}
    <AutoRepairSection receipt={scope === 'user' ? source.user_repair : source.workspace_repair}
      malformed={!config} focused={repairFocused} inspection={inspection} presented={repairPresented} />

    <Button disabled={busy || !targetValid} onClick={() => actor.send({ type: 'RECONCILE' })}>{tx('settings:advanced-page.rescan-configuration-files')}</Button>
  </section>;
}

/** The semantic-unit editors of this scope's parsed `rustx.toml`. */
function AdvancedEditors({ source, scope, document, revision }: {
  source: SourceSettings; scope: SourceScope; document: RuntimeLayer; revision: string;
}) {
  const tx = useTranslation();
  const [environment, setEnvironment] = useState('');
  const [selectedEnvironment, selectEnvironment] = useState('');
  const resolved = source.resolved;
  return <>
    <h4>{tx('settings:advanced-page.context-and-runtime-limits')}</h4>
    <UnitForm<TimeoutLayer> title={tx('settings:advanced-page.model-timeout')} authored={document.model_timeout_policy ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'model_timeout', authored } })}>
      {(value, change) => <>{(['response_start_timeout_ms', 'stream_idle_timeout_ms'] as const).map(key =>
        <TextField key={key} label={tx(key === 'response_start_timeout_ms' ? 'settings:advanced.response-timeout' : 'settings:advanced.stream-timeout')} value={value[key]} change={next => change({ ...value, [key]: next || null })} />)}</>}
    </UnitForm>

    <UnitForm<ToolDeadlineLayer> title={tx('settings:advanced-page.tool-deadline')} authored={document.tool_deadline_policy ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'tool_deadline', authored } })}>
      {(value, change) => <>
        <TextField label={tx('settings:advanced-page.hard-deadline-ms')} value={value.hard_deadline_ms} change={hard_deadline_ms => change({ ...value, hard_deadline_ms: hard_deadline_ms || null })} />
        <Choice label={tx('settings:advanced-page.idle-liveness')} value={value.idle_liveness_ms?.mode ?? ''}
          options={[['', tx('settings:copy.domain-default')], ['disabled', tx('settings:copy.disabled')], ['window', tx('settings:copy.idle-window')]]}
          onChange={mode => change({ ...value, idle_liveness_ms: mode === 'window' ? { mode: 'window', milliseconds: '30000' } : mode === 'disabled' ? { mode: 'disabled' } : null })} />
        {value.idle_liveness_ms?.mode === 'window' && <TextField label={tx('settings:advanced-page.idle-window-ms')} value={value.idle_liveness_ms.milliseconds}
          change={milliseconds => change({ ...value, idle_liveness_ms: { mode: 'window', milliseconds } })} />}
      </>}
    </UnitForm>

    <UnitForm<SubagentsLayer> title={tx('settings:advanced-page.child-capacity')} authored={document.subagents ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'capacity', authored } })}>
      {(value, change) => <label>{tx('settings:advanced-page.maximum-concurrent-children')}<input type="number" min="1" value={value.max_concurrent ?? ''}
        onChange={event => change({ max_concurrent: event.target.value ? Number(event.target.value) : null })} /></label>}
    </UnitForm>

    <h4>{tx('settings:advanced-page.environment')}</h4>
    {/* Native projects environment *identities* only: `RuntimeLayer.environment`
        is a list of names on the wire, so an inherited value cannot be
        enumerated, displayed or copied here even in principle. */}
    <div className={css.actions}>
      <TextField label={tx('settings:advanced-page.environment-variable-identity')} value={environment} change={setEnvironment} />
      <Button disabled={!environment} onClick={() => selectEnvironment(environment)}>{tx('settings:advanced-page.edit-environment-variable')}</Button>
    </div>
    <ul>{reachableEnvironment(scope, document.environment, resolved?.environment).map(name =>
      <li key={name}><Button onClick={() => selectEnvironment(name)}>{name}</Button></li>)}</ul>
    {selectedEnvironment && <UnitForm<string> key={selectedEnvironment} title={tx('settings:advanced-page.environment-value', { p0: selectedEnvironment })}
      authored={document.environment?.includes(selectedEnvironment) ? '' : undefined} blank="" revision={revision} redacted
      mutation={authored => ({ kind: 'config', mutation: { unit: 'environment', name: selectedEnvironment, authored } })}>
      {(value, change) => <TextField label={tx('settings:advanced-page.literal-tool-environment-value')} secret value={value} change={change} />}
    </UnitForm>}

  </>;
}
