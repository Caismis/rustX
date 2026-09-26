import { displayText } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import { useState, useSyncExternalStore } from 'react';
import type { ConnectionController } from '../../connection/controller';
import type { AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';

export function ConnectionSettings({ connection, client }: { connection: ConnectionController; client: AppServerClient }) {
  const tx = useTranslation();
  const selection = useSyncExternalStore(connection.subscribe, connection.getSnapshot);
  const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const [endpoint, setEndpoint] = useState(''), [token, setToken] = useState('');
  return <section className="connection-form" aria-label={tx('settings:connection-settings.connection-settings')}>
    <h2>{tx('settings:connection-settings.connection')}</h2>
    <label>{tx('settings:connection-settings.mode')}<select aria-label={tx('settings:connection-settings.connection-mode')} value={selection.selectedMode} onChange={event => { setToken(''); void connection.select(event.target.value as 'local' | 'remote'); }}>
      <option value="local">{tx('settings:connection-settings.local-managed-connection')}</option><option value="remote">{tx('settings:connection-settings.remote-app-server')}</option>
    </select></label>
    <p role="status">{tx('settings:connection-settings.active-mode')}{' '}{selection.mode === 'local' ? tx('settings:connection-settings.local-managed-connection') : tx('settings:connection-settings.remote-app-server')}</p>
    <p className="connection-status">{transport.connection === 'connected' ? tx('settings:connection-settings.connected') : selection.busy ? tx('settings:connection-settings.connecting') : tx('settings:connection-settings.disconnected')}</p>
    {selection.selectedMode === 'remote' ? <>
      <label>{tx('settings:connection-settings.websocket-endpoint')}<Input aria-label={tx('settings:connection-settings.websocket-endpoint')} value={endpoint} onChange={event => setEndpoint(event.target.value)} /></label>
      <label>{tx('settings:connection-settings.transport-token')}<Input type="password" autoComplete="off" aria-label={tx('settings:connection-settings.transport-token')} value={token} onChange={event => setToken(event.target.value)} /></label>
      <p>{tx('settings:connection-settings.transport-credentials-stay-in-page-memory-remote-attachment-gran')}</p>
      <Button disabled={selection.busy} onClick={() => void connection.connectRemote(endpoint, token)}>{tx('settings:connection-settings.connect')}</Button>
    </> : <p>{tx('settings:connection-settings.the-launcher-supplies-this-connection-automatically')}</p>}
    <Button disabled={selection.busy} onClick={() => void connection.reconnect()}>{tx('settings:connection-settings.reconnect')}</Button>
    <Button onClick={() => void connection.disconnect()}>{tx('settings:connection-settings.disconnect')}</Button>
    {(selection.error || transport.error) && <p role="alert">{displayText(tx, selection.error || transport.error || '')}</p>}
    <details><summary>{tx('settings:connection-settings.connection-details')}</summary><p>{transport.endpoint} {tx('settings:connection-settings.generation')} {transport.generation} {tx('settings:copy.app-server-v23')}</p></details>
    {!!transport.detached?.length && <details><summary>{tx('settings:connection-settings.detached-authority-diagnostics')}</summary>
      <p>{tx('settings:connection-settings.historical-evidence-only-these-operations-are-never-replayed-and')}</p>
      {transport.detached.map((evidence, index) => <section key={index}><h3>{evidence.authority}</h3>
        <pre>{JSON.stringify(evidence, null, 2)}</pre>
        <Button onClick={() => client.acknowledgeDetached(index)}>{tx('settings:connection-settings.i-have-reviewed-this-historical-evidence')}</Button>
      </section>)}
    </details>}
    {!!transport.uncertain.filter(item => !item.interactionKey).length && <details><summary>{tx('settings:connection-settings.review-uncertain-operations')}</summary>
      <p>{tx('settings:connection-settings.verify-the-affected-work-in-developer-inspector-before-acknowled')}</p>
      {transport.uncertain.filter(item => !item.interactionKey).map(item => <div key={item.id}><p>{item.method} {tx('settings:connection-settings.request')}{' '}{item.id}</p><Button onClick={() => client.acknowledgeDiagnostic(item.id)}>{tx('settings:connection-settings.i-have-verified-the-affected-work')}</Button></div>)}
    </details>}
  </section>;
}
