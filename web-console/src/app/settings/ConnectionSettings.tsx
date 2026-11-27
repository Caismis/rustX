import { useId, useState, useSyncExternalStore } from 'react';
import { displayText } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import type { ConnectionController } from '../../connection/controller';
import type { AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';
import { EnumSelect } from '../../presentation/primitives/EnumSelect';
import { StateDot } from '../../presentation/primitives/StateDot';
import { SettingsGroup, SettingsRow } from '../../presentation/settings/SettingsPageShell';
import { SettingsNotice } from './SettingsFeedback';

export function ConnectionSettings({ connection, client }: { connection: ConnectionController; client: AppServerClient }) {
 const tx = useTranslation();
 const selection = useSyncExternalStore(connection.subscribe, connection.getSnapshot);
 const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
 const [endpoint, setEndpoint] = useState('');
 const [token, setToken] = useState('');
 const modeId = useId();
 const endpointId = useId();
 const tokenId = useId();

 const statusText = transport.connection === 'connected'
 ? tx('settings:connection-settings.connected')
 : selection.busy
 ? tx('settings:connection-settings.connecting')
 : tx('settings:connection-settings.disconnected');
 const dotState = transport.connection === 'connected' ? 'done' : selection.busy ? 'ongoing' : 'idle';
 const hasDiagnostics = !!transport.detached?.length
 || (['disconnected', 'stale', 'error'].includes(transport.connection)
 && Object.values(transport.views).some(view => view.deleting || view.error || view.modelMutation || view.cancellation))
 || !!transport.uncertain.filter(item => !item.interactionKey).length;

 return (
 <section className="flex flex-col gap-[20px] min-w-0" aria-label={tx('settings:connection-settings.connection-settings')}>
 <SettingsGroup>
 <SettingsRow
 label={tx('settings:connection-settings.connection')}
 description={selection.mode !== selection.selectedMode
 ? `${tx('settings:connection-settings.active-mode')} ${selection.mode === 'local' ? tx('settings:connection-settings.local-managed-connection') : tx('settings:connection-settings.remote-app-server')}`
 : undefined}
 control={
 <span className="inline-flex items-center gap-[7px] h-[26px] px-[11px] py-0 [border:1px_solid_var(--dsw-alias-border-l2)] rounded-full bg-[var(--dsw-alias-bg-base)] text-[12.5px] font-medium leading-[18px] [color:var(--dsw-alias-label-primary)] data-[connection=connected]:[border-color:color-mix(in_srgb,var(--dsw-alias-state-success-primary)_45%,var(--dsw-alias-border-l2))]" data-connection={transport.connection}>
 <StateDot state={dotState} size={8} />
 <span className="connection-status">{statusText}</span>
 </span>
 }
 />
 <SettingsRow
 vertical
 label={<label htmlFor={modeId} className="inline! p-0!! [color:inherit]!">{tx('settings:connection-settings.mode')}</label>}
 >
 <EnumSelect
 id={modeId}
 className="appearance-none pr-[36px]! cursor-pointer"
 label={tx('settings:connection-settings.connection-mode')}
 value={selection.selectedMode}
 onChange={value => { setToken(''); void connection.select(value); }}
 options={[
 { value: 'local', label: tx('settings:connection-settings.local-managed-connection') },
 { value: 'remote', label: tx('settings:connection-settings.remote-app-server') },
 ]}
 />
 </SettingsRow>
 {selection.selectedMode === 'remote' ? (
 <>
 <SettingsRow
 vertical
 label={<label htmlFor={endpointId} className="inline! p-0!! [color:inherit]!">{tx('settings:connection-settings.websocket-endpoint')}</label>}
 >
 <Input
 id={endpointId}
 className="w-full"
 aria-label={tx('settings:connection-settings.websocket-endpoint')}
 value={endpoint}
 onChange={event => setEndpoint(event.target.value)}
 />
 </SettingsRow>
 <SettingsRow
 vertical
 label={<label htmlFor={tokenId} className="inline! p-0!! [color:inherit]!">{tx('settings:connection-settings.transport-token')}</label>}
 >
 <Input
 id={tokenId}
 className="w-full"
 type="password"
 autoComplete="off"
 aria-label={tx('settings:connection-settings.transport-token')}
 value={token}
 onChange={event => setToken(event.target.value)}
 />
 </SettingsRow>
 </>
 ) : (
 <SettingsRow description={tx('settings:connection-settings.the-launcher-supplies-this-connection-automatically')} />
 )}
 <div className="flex flex-wrap items-center gap-[8px] px-[16px] py-[14px]">
 {selection.selectedMode === 'remote' && (
 <Button variant="primary" size="sm" disabled={selection.busy} onClick={() => void connection.connectRemote(endpoint, token)}>
 {tx('settings:connection-settings.connect')}
 </Button>
 )}
 <Button variant="outline" size="sm" disabled={selection.busy} onClick={() => void connection.reconnect()}>
 {tx('settings:connection-settings.reconnect')}
 </Button>
 <Button variant="danger-outline" size="sm" onClick={() => void connection.disconnect()}>
 {tx('settings:connection-settings.disconnect')}
 </Button>
 </div>
 </SettingsGroup>

 <SettingsNotice text={displayText(tx, selection.error || transport.error || '')} />

 {hasDiagnostics && (
 <SettingsGroup>
 <div className="px-[16px] py-[6px]">
 {!!transport.detached?.length && <details><summary>{tx('settings:connection-settings.detached-authority-diagnostics')}</summary>
 <p>{tx('settings:connection-settings.historical-evidence-only-these-operations-are-never-replayed-and')}</p>
 {transport.detached.map((evidence, index) => <section key={index}><h3>{tx('settings:connection.previous-connection', { number: index + 1 })}</h3>
 {evidence.sessions.filter(row => !row.deletion).map(row => <p key={row.id}>{tx('settings:connection.review-conversation', { id: row.id })}{row.error && `: ${displayText(tx, row.error)}`}</p>)}
 {evidence.sessions.filter(row => row.deletion).map(row => <p key={row.id}>{row.id}: {tx(row.deletion === 'uncertain' ? 'settings:connection-settings.deletion-uncertain' : row.deletion === 'committed_cleanup_pending' ? 'settings:connection-settings.deletion-cleanup' : 'settings:connection-settings.deletion-durability')}</p>)}
 {!!evidence.operations.length && <p>{tx('settings:connection.pending-count', { count: evidence.operations.length })}</p>}
 <Button size="sm" variant="outline" onClick={() => client.acknowledgeDetached(index)}>{tx('settings:connection-settings.i-have-reviewed-this-historical-evidence')}</Button>
 </section>)}
 </details>}
 {['disconnected', 'stale', 'error'].includes(transport.connection) && Object.values(transport.views).some(view => view.deleting || view.error || view.modelMutation || view.cancellation) && <details><summary>{tx('settings:connection-settings.current-evidence')}</summary>
 <p>{tx('settings:connection-settings.historical-evidence-only-these-operations-are-never-replayed-and')}</p>
 {Object.values(transport.views).filter(view => view.deleting || view.error || view.modelMutation || view.cancellation).map(view => <section key={view.id}>
 <p>{tx('settings:connection.review-conversation', { id: view.id })}</p>
 {view.error && <p>{displayText(tx, view.error)}</p>}
 {view.deleting && <p>{tx(!view.deletionCommitted ? 'settings:connection-settings.deletion-uncertain' : view.deletionCommitted === 'committed_cleanup_pending' ? 'settings:connection-settings.deletion-cleanup' : 'settings:connection-settings.deletion-durability')}</p>}
 {view.modelMutation && <p>{tx('settings:connection.review-model-change')}</p>}
 {view.cancellation && <p>{tx('settings:connection.review-cancellation')}</p>}
 <Button size="sm" variant="outline" onClick={() => client.acknowledgeSessionDiagnostic(view.id)}>{tx('settings:connection-settings.i-have-reviewed-this-historical-evidence')}</Button>
 </section>)}
 </details>}
 {!!transport.uncertain.filter(item => !item.interactionKey).length && <details><summary>{tx('settings:connection-settings.review-uncertain-operations')}</summary>
 <p>{tx('settings:connection-settings.verify-the-affected-work-before-acknowledging')}</p>
 {transport.uncertain.filter(item => !item.interactionKey).map((item, index) => <div key={item.id}><p>{item.sessionId ? tx('settings:connection.review-conversation', { id: item.sessionId }) : tx('settings:connection.pending-operation', { number: index + 1 })}</p>{item.diagnostic && <p>{displayText(tx, item.diagnostic)}</p>}<Button size="sm" variant="outline" onClick={() => client.acknowledgeDiagnostic(item.id)}>{tx('settings:connection-settings.i-have-verified-the-affected-work')}</Button></div>)}
 </details>}
 </div>
 </SettingsGroup>
 )}
 </section>
 );
}
