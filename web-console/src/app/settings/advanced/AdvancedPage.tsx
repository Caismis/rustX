import { useTranslation } from '../../../locale/react';
import { useState, type ReactNode } from 'react';
import type {
  ContextLayer, RuntimeLayer, SourceMutation, SourceScope, SourceSettings, SubagentsLayer,
  TimeoutLayer, ToolDeadlineLayer,
} from '../../../../../protocol/app-server/v80';
import { Button } from '../../../presentation/primitives/Button';
import { UnitForm, PageTransactionFooter } from '../forms/bridge';
import { TextField } from '../forms/controls';
import { Choice } from '../primitives/aria';
import { reachableEnvironment } from '../projection';
import { useSettingsActor } from '../machines/react';
import { AutoRepairSection } from './AutoRepair';
import { SettingsPageShell, SettingsGroup, SettingsRow } from '../../../presentation/settings/SettingsPageShell';
import { PageScopeToolbar, PageRefreshButton } from '../ScopeToolbar';
import { WorkspaceFollowGlobal } from '../WorkspaceFollowGlobal';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** User-adjustable runtime limits and environment values. Source revisions,
 * raw projections and process internals are not settings controls. */
export function AdvancedPage({
  source,
  config,
  closed,
  scope,
  busy,
  targetValid,
  repairFocused = false,
  inspection,
  repairPresented,
  openConnection,
  bodyDisabled = false,
}: {
  source: SourceSettings;
  scope: SourceScope;
  /** This scope's parsed `rustx.toml` and its exact revision, when it parses. */
  config?: { document: RuntimeLayer; revision: string };
  /** What stands in for the editors while it does not. */
  closed?: ReactNode;
  busy: boolean;
  targetValid: boolean;
  /** Opened at Auto Repair, from a repair notification's View details. */
  repairFocused?: boolean;
  /** The View details inspection awaiting these details, if any. */
  inspection?: unknown;
  /** Reports Auto Repair details presented from a current authoritative
   * read; absent while `source` is only stale presentation data. */
  repairPresented?: (receiptId: string) => void;
  /** Opens this owner's Connection view, when navigation admits it. */
  openConnection?: () => void;
  bodyDisabled?: boolean;
}) {
  const tx = useTranslation();
  const actor = useSettingsActor();
  return (
    <SettingsPageShell
      className={`${css.settings} ${css.page}`}
      title={tx('settings:advanced-page.advanced')}
      description={tx('settings:advanced-page.runtime-limits-environment-process-policy-and-the-native-diagnos')}
      bodyDisabled={bodyDisabled}
      toolbar={<><PageScopeToolbar /><PageRefreshButton /></>}
      footer={<PageTransactionFooter />}
    >
      {openConnection && (
        <SettingsGroup title={tx('settings:connection-settings.connection')}>
          <SettingsRow
            label={tx('settings:connection-settings.connection')}
            description={tx('settings:connection-settings.connection-settings')}
            control={
              <Button variant="outline" size="sm" onClick={openConnection}>
                {tx('settings:connection-settings.connection')}
              </Button>
            }
          />
        </SettingsGroup>
      )}

      {config
        ? <AdvancedEditors source={source} scope={scope} document={config.document} revision={config.revision} />
        : closed}

      <SettingsGroup title={tx('settings:advanced-page.advanced')}>
        <div style={{ padding: '14px 16px', display: 'flex', flexDirection: 'column', gap: 12 }}>
          <AutoRepairSection
            receipt={scope === 'user' ? source.user_repair : source.workspace_repair}
            malformed={!config}
            focused={repairFocused}
            inspection={inspection}
            presented={repairPresented}
          />
          <div>
            <Button variant="outline" size="sm" disabled={busy || !targetValid} onClick={() => actor.send({ type: 'RECONCILE' })}>
              {tx('settings:advanced-page.rescan-configuration-files')}
            </Button>
          </div>
        </div>
      </SettingsGroup>
    </SettingsPageShell>
  );
}

/** The semantic-unit editors of this scope's parsed `rustx.toml`. */
function AdvancedEditors({ source, scope, document, revision }: {
  source: SourceSettings; scope: SourceScope; document: RuntimeLayer; revision: string;
}) {
  const tx = useTranslation();
  const [environment, setEnvironment] = useState('');
  const [selectedEnvironment, selectEnvironment] = useState('');
  const resolved = source.resolved;

  const hasWorkspaceOverrides = scope === 'workspace' && (
    document.context != null
    || document.model_timeout_policy != null
    || document.tool_deadline_policy != null
    || document.subagents != null
    || (document.environment?.length ?? 0) > 0
  );
  const workspaceRemovals: SourceMutation[] = scope === 'workspace' ? [
    ...(document.context != null ? [{ kind: 'config', mutation: { unit: 'context', authored: null } } as const] : []),
    ...(document.model_timeout_policy != null ? [{ kind: 'config', mutation: { unit: 'model_timeout', authored: null } } as const] : []),
    ...(document.tool_deadline_policy != null ? [{ kind: 'config', mutation: { unit: 'tool_deadline', authored: null } } as const] : []),
    ...(document.subagents != null ? [{ kind: 'config', mutation: { unit: 'capacity', authored: null } } as const] : []),
    ...(document.environment ?? []).map(name => ({ kind: 'config', mutation: { unit: 'environment', name, authored: null } } as const)),
  ] : [];

  return (
    <WorkspaceFollowGlobal scope={scope} hasOverrides={hasWorkspaceOverrides} authoredRemovals={workspaceRemovals} units={['context', 'model_timeout', 'tool_deadline', 'capacity', 'environment']}>
      <SettingsGroup title={tx('settings:advanced-page.context-policy')}>
        <UnitForm<ContextLayer>
          title={tx('settings:advanced-page.context-policy')}
          legendHidden
          authored={document.context ?? undefined}
          blank={{}}
          revision={revision}
          mutation={authored => ({ kind: 'config', mutation: { unit: 'context', authored } })}
        >
          {(value, change) => (
            <div className={css.grid}>
              {(['reserve_tokens', 'keep_recent_tokens'] as const).map(key => (
                <TextField
                  key={key}
                  label={tx(key === 'reserve_tokens' ? 'settings:advanced.reserve-tokens' : 'settings:advanced.recent-tokens')}
                  value={value[key]}
                  change={next => change({ ...value, [key]: next || null })}
                />
              ))}
              <Choice
                label={tx('settings:advanced-page.summary-output')}
                value={value.summary_output_cap?.mode ?? ''}
                options={[['', tx('settings:copy.domain-default')], ['model_limit', tx('settings:copy.model-limit')], ['limit', tx('settings:copy.explicit-token-limit')]]}
                onChange={mode => change({ ...value, summary_output_cap: mode === 'limit' ? { mode: 'limit', tokens: 2048 } : mode === 'model_limit' ? { mode: 'model_limit' } : null })}
              />
              {value.summary_output_cap?.mode === 'limit' && (
                <label>
                  {tx('settings:advanced-page.summary-token-limit')}
                  <input
                    type="number"
                    min="1"
                    value={value.summary_output_cap.tokens}
                    onChange={event => change({ ...value, summary_output_cap: { mode: 'limit', tokens: Number(event.target.value) } })}
                  />
                </label>
              )}
            </div>
          )}
        </UnitForm>
      </SettingsGroup>

      <SettingsGroup title={tx('settings:advanced-page.context-and-runtime-limits')}>
        <UnitForm<TimeoutLayer>
          title={tx('settings:advanced-page.model-timeout')}
          authored={document.model_timeout_policy ?? undefined}
          blank={{}}
          revision={revision}
          mutation={authored => ({ kind: 'config', mutation: { unit: 'model_timeout', authored } })}
        >
          {(value, change) => (
            <div className={css.grid}>
              {(['response_start_timeout_ms', 'stream_idle_timeout_ms'] as const).map(key => (
                <TextField
                  key={key}
                  label={tx(key === 'response_start_timeout_ms' ? 'settings:advanced.response-timeout' : 'settings:advanced.stream-timeout')}
                  value={value[key]}
                  change={next => change({ ...value, [key]: next || null })}
                />
              ))}
            </div>
          )}
        </UnitForm>

        <UnitForm<ToolDeadlineLayer>
          title={tx('settings:advanced-page.tool-deadline')}
          authored={document.tool_deadline_policy ?? undefined}
          blank={{}}
          revision={revision}
          mutation={authored => ({ kind: 'config', mutation: { unit: 'tool_deadline', authored } })}
        >
          {(value, change) => (
            <div className={css.grid}>
              <TextField
                label={tx('settings:advanced-page.hard-deadline-ms')}
                value={value.hard_deadline_ms}
                change={hard_deadline_ms => change({ ...value, hard_deadline_ms: hard_deadline_ms || null })}
              />
              <Choice
                label={tx('settings:advanced-page.idle-liveness')}
                value={value.idle_liveness_ms?.mode ?? ''}
                options={[['', tx('settings:copy.domain-default')], ['disabled', tx('settings:copy.disabled')], ['window', tx('settings:copy.idle-window')]]}
                onChange={mode => change({ ...value, idle_liveness_ms: mode === 'window' ? { mode: 'window', milliseconds: '30000' } : mode === 'disabled' ? { mode: 'disabled' } : null })}
              />
              {value.idle_liveness_ms?.mode === 'window' && (
                <TextField
                  label={tx('settings:advanced-page.idle-window-ms')}
                  value={value.idle_liveness_ms.milliseconds}
                  change={milliseconds => change({ ...value, idle_liveness_ms: { mode: 'window', milliseconds } })}
                />
              )}
            </div>
          )}
        </UnitForm>

        <UnitForm<SubagentsLayer>
          title={tx('settings:advanced-page.child-capacity')}
          authored={document.subagents ?? undefined}
          blank={{}}
          revision={revision}
          mutation={authored => ({ kind: 'config', mutation: { unit: 'capacity', authored } })}
        >
          {(value, change) => (
            <label>
              {tx('settings:advanced-page.maximum-concurrent-children')}
              <input
                type="number"
                min="1"
                value={value.max_concurrent ?? ''}
                onChange={event => change({ max_concurrent: event.target.value ? Number(event.target.value) : null })}
              />
            </label>
          )}
        </UnitForm>
      </SettingsGroup>

      <SettingsGroup title={tx('settings:advanced-page.environment')}>
        <div style={{ padding: '14px 16px' }}>
          <div className={css.actions} style={{ marginTop: 0 }}>
            <TextField label={tx('settings:advanced-page.environment-variable-identity')} value={environment} change={setEnvironment} />
            <Button size="sm" variant="outline" disabled={!environment} onClick={() => selectEnvironment(environment)}>
              {tx('settings:advanced-page.edit-environment-variable')}
            </Button>
          </div>
          {!!reachableEnvironment(scope, document.environment, resolved?.environment).length && (
            <ul style={{ marginTop: 10 }}>
              {reachableEnvironment(scope, document.environment, resolved?.environment).map(name => (
                <li key={name}>
                  <Button variant="ghost" size="sm" onClick={() => selectEnvironment(name)}>{name}</Button>
                </li>
              ))}
            </ul>
          )}
        </div>
        {selectedEnvironment && (
          <UnitForm<string>
            key={selectedEnvironment}
            title={tx('settings:advanced-page.environment-value', { p0: selectedEnvironment })}
            authored={document.environment?.includes(selectedEnvironment) ? '' : undefined}
            blank=""
            revision={revision}
            redacted
            mutation={authored => ({ kind: 'config', mutation: { unit: 'environment', name: selectedEnvironment, authored } })}
          >
            {(value, change) => (
              <TextField label={tx('settings:advanced-page.literal-tool-environment-value')} secret value={value} change={change} />
            )}
          </UnitForm>
        )}
      </SettingsGroup>
    </WorkspaceFollowGlobal>
  );
}
