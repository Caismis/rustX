import type {
  AgentStatusExtensionDocument, GoalExtensionDocument, SourceScope, SourceSettings, TodoExtensionDocument,
} from '../../../../../protocol/app-server/v20';
import { UnitForm } from '../forms/bridge';
import { TextField } from '../forms/controls';
import { Advanced, Choice, Toggle } from '../primitives/aria';
import { sourceView } from '../projection';
import { ConfigUnavailable } from './ExtensionDetail';
import css from '../../../presentation/settings/SettingsContent.module.css';

/** The native extensions: Todo, Goal and Agent Status.
 *
 * These are not resource documents and not installable plugins — each is a
 * `rustx.toml` semantic unit, so authoring one and enabling it are the same
 * mutation. There is deliberately no marketplace, no plugin runtime and no
 * installation flow here, and the ambiguous top-level "Plugins" product
 * language is gone.
 *
 * What this page configures is composition, not conversation data. A
 * Conversation's current Todo list and its Goal are runtime state owned by
 * that Conversation; disabling an extension here composes it out of future
 * runtimes and never edits or erases the history a Conversation already
 * holds. */
export function NativeExtensions({ source, scope, revision }: { source: SourceSettings; scope: SourceScope; revision?: string }) {
  if (revision === undefined) return <section aria-label="Native extensions"><h4>Native extensions</h4><ConfigUnavailable /></section>;
  const plugins = sourceView(source, scope)?.authored?.agent?.plugins;
  return <section aria-label="Native extensions">
    <h4>Native extensions</h4>
    <p>Closed native extensions, configured as part of this source. They default to off, and a Workspace replaces the same User unit as a whole.</p>
    <p className={css.hint}>Current Todo and Goal contents belong to the Conversation that owns them. Turning an extension off composes it out of new runtimes; it never edits conversation history.</p>

    <UnitForm<TodoExtensionDocument> title="Todo extension" authored={plugins?.todo ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'todo', authored } })}>
      {(value, change) => <Toggle label="Enable Todo" checked={value.enabled ?? false} onChange={enabled => change({ enabled })} />}
    </UnitForm>

    <UnitForm<GoalExtensionDocument> title="Goal extension" authored={plugins?.goal ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'goal', authored } })}>
      {(value, change) => <Toggle label="Enable Goal" checked={value.enabled ?? false} onChange={enabled => change({ enabled })} />}
    </UnitForm>

    <UnitForm<AgentStatusExtensionDocument> title="Agent Status extension" authored={plugins?.agent_status ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'agent_status', authored } })}>
      {(value, change) => <StatusFields value={value} change={change} />}
    </UnitForm>
  </section>;
}

export function StatusFields({ value, change }: { value: AgentStatusExtensionDocument; change: (value: AgentStatusExtensionDocument) => void }) {
  return <>
    <Toggle label="Enable Agent Status" checked={value.enabled ?? false} onChange={enabled => change({ ...value, enabled })} />
    <Advanced title="Status contributors">
      <Choice label="Time contributor" value={value.time?.enabled === undefined ? '' : String(value.time.enabled)}
        options={[['', 'Native default'], ['true', 'On'], ['false', 'Off']]}
        onChange={next => change({ ...value, time: { ...value.time, enabled: next === '' ? undefined : next === 'true' } })} />
      <TextField label="Time zone (IANA)" value={value.time?.timezone ?? ''}
        change={timezone => change({ ...value, time: { ...value.time, timezone: timezone || null } })} />
      <Choice label="Background contributor" value={value.background?.enabled === undefined ? '' : String(value.background.enabled)}
        options={[['', 'Native default'], ['true', 'On'], ['false', 'Off']]}
        onChange={next => change({ ...value, background: { ...value.background, enabled: next === '' ? undefined : next === 'true' } })} />
    </Advanced>
  </>;
}
