import { useState, useSyncExternalStore } from 'react';
import type { ConnectionController } from '../../connection/controller';
import type { AppServerClient } from '../../client/app-server';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';

export function ConnectionSettings({ connection, client }: { connection: ConnectionController; client: AppServerClient }) {
  const selection = useSyncExternalStore(connection.subscribe, connection.getSnapshot);
  const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const [endpoint, setEndpoint] = useState(''), [token, setToken] = useState('');
  return <section className="connection-form" aria-label="Connection Settings">
    <h2>Connection</h2>
    <label>Mode<select aria-label="Connection mode" value={selection.selectedMode} onChange={event => { setToken(''); void connection.select(event.target.value as 'local' | 'remote'); }}>
      <option value="local">Local managed connection</option><option value="remote">Remote App Server</option>
    </select></label>
    <p role="status">Active mode: {selection.mode === 'local' ? 'Local managed connection' : 'Remote App Server'}</p>
    <p className="connection-status">{transport.connection === 'connected' ? 'Connected' : selection.busy ? 'Connecting…' : 'Disconnected'}</p>
    {selection.selectedMode === 'remote' ? <>
      <label>WebSocket endpoint<Input aria-label="WebSocket endpoint" value={endpoint} onChange={event => setEndpoint(event.target.value)} /></label>
      <label>Transport token<Input type="password" autoComplete="off" aria-label="Transport token" value={token} onChange={event => setToken(event.target.value)} /></label>
      <p>Transport credentials stay in page memory. Remote attachment grants no Workspace filesystem access.</p>
      <Button disabled={selection.busy} onClick={() => void connection.connectRemote(endpoint, token)}>Connect</Button>
    </> : <p>The launcher supplies this connection automatically.</p>}
    <Button disabled={selection.busy} onClick={() => void connection.reconnect()}>Reconnect</Button>
    <Button onClick={() => void connection.disconnect()}>Disconnect</Button>
    {(selection.error || transport.error) && <p role="alert">{selection.error || transport.error}</p>}
    <details><summary>Connection details</summary><p>{transport.endpoint} · generation {transport.generation} · App Server v15</p></details>
    {!!transport.detached?.length && <details><summary>Detached authority diagnostics</summary>
      <p>Historical evidence only. These operations are never replayed and cannot control the current server. Verify the old server separately before acknowledging.</p>
      {transport.detached.map((evidence, index) => <section key={index}><h3>{evidence.authority}</h3>
        <pre>{JSON.stringify(evidence, null, 2)}</pre>
        <Button onClick={() => client.acknowledgeDetached(index)}>I have reviewed this historical evidence</Button>
      </section>)}
    </details>}
    {!!transport.uncertain.filter(item => !item.interactionKey).length && <details><summary>Review uncertain operations</summary>
      <p>Verify the affected work in Developer Inspector before acknowledging. Nothing is replayed.</p>
      {transport.uncertain.filter(item => !item.interactionKey).map(item => <div key={item.id}><p>{item.method} · request {item.id}</p><Button onClick={() => client.acknowledgeDiagnostic(item.id)}>I have verified the affected work</Button></div>)}
    </details>}
  </section>;
}
