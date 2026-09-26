import { useTranslation } from '../../../locale/react';
import type {
  AgentStatusExtensionDocument, GoalExtensionDocument, SourceScope, SourceSettings, TodoExtensionDocument,
} from '../../../../../protocol/app-server/v23';
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
  const tx = useTranslation();
  if (revision === undefined) return <section aria-label={tx('settings:native-extensions.native-extensions')}><h4>{tx('settings:native-extensions.native-extensions')}</h4><ConfigUnavailable /></section>;
  const plugins = sourceView(source, scope)?.authored?.agent?.plugins;
  return <section aria-label={tx('settings:native-extensions.native-extensions')}>
    <h4>{tx('settings:native-extensions.native-extensions')}</h4>
    <p>{tx('settings:native-extensions.closed-native-extensions-configured-as-part-of-this-source-they')}</p>
    <p className={css.hint}>{tx('settings:native-extensions.current-todo-and-goal-contents-belong-to-the-conversation-that-o')}</p>

    <UnitForm<TodoExtensionDocument> title={tx('settings:native-extensions.todo-extension')} authored={plugins?.todo ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'todo', authored } })}>
      {(value, change) => <Toggle label={tx('settings:native-extensions.enable-todo')} checked={value.enabled ?? false} onChange={enabled => change({ enabled })} />}
    </UnitForm>

    <UnitForm<GoalExtensionDocument> title={tx('settings:native-extensions.goal-extension')} authored={plugins?.goal ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'goal', authored } })}>
      {(value, change) => <Toggle label={tx('settings:native-extensions.enable-goal')} checked={value.enabled ?? false} onChange={enabled => change({ enabled })} />}
    </UnitForm>

    <UnitForm<AgentStatusExtensionDocument> title={tx('settings:native-extensions.agent-status-extension')} authored={plugins?.agent_status ?? undefined} blank={{}} revision={revision}
      mutation={authored => ({ kind: 'config', mutation: { unit: 'agent_status', authored } })}>
      {(value, change) => <StatusFields value={value} change={change} />}
    </UnitForm>
  </section>;
}

export function StatusFields({ value, change }: { value: AgentStatusExtensionDocument; change: (value: AgentStatusExtensionDocument) => void }) {
  const tx = useTranslation();
  return <>
    <Toggle label={tx('settings:native-extensions.enable-agent-status')} checked={value.enabled ?? false} onChange={enabled => change({ ...value, enabled })} />
    <Advanced title={tx('settings:native-extensions.status-contributors')}>
      <Choice label={tx('settings:native-extensions.time-contributor')} value={value.time?.enabled === undefined ? '' : String(value.time.enabled)}
        options={[['', tx('settings:copy.native-default')], ['true', tx('settings:copy.on')], ['false', tx('settings:copy.off')]]}
        onChange={next => change({ ...value, time: { ...value.time, enabled: next === '' ? undefined : next === 'true' } })} />
      <TextField label={tx('settings:native-extensions.time-zone-iana')} value={value.time?.timezone ?? ''}
        change={timezone => change({ ...value, time: { ...value.time, timezone: timezone || null } })} />
      <Choice label={tx('settings:native-extensions.background-contributor')} value={value.background?.enabled === undefined ? '' : String(value.background.enabled)}
        options={[['', tx('settings:copy.native-default')], ['true', tx('settings:copy.on')], ['false', tx('settings:copy.off')]]}
        onChange={next => change({ ...value, background: { ...value.background, enabled: next === '' ? undefined : next === 'true' } })} />
    </Advanced>
  </>;
}
