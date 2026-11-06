/* Copyright (c) 2026 DeepSeek. MIT. Timing adapted from SettingsRoot; see PROVENANCE.md. */
import { useLayoutEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { AppServerClient } from '../client/app-server';
import { useClientSelector, sameValue } from '../client/selectors';
import type { ConnectionController } from '../connection/controller';
import { useTranslation } from '../locale/react';
import { ConnectionIndicator } from '../presentation/primitives/ConnectionIndicator';
import { IconInfoOutline14 } from '../presentation/primitives/icons';
import { Tooltip } from '../presentation/primitives/Tooltip';
import css from './ConnectionFeedback.module.css';

/** Display timing never delays native readiness or initiates a reconnect. */
export function ConnectionFeedback({ client, connection, wide, settings }: {
  client: AppServerClient; connection: ConnectionController; wide: boolean; settings: () => void;
}) {
  const tx = useTranslation();
  const transport = useClientSelector(client, state => state.connection, sameValue);
  const selection = useSyncExternalStore(connection.subscribe, connection.getSnapshot);
  const phase = selection.busy || ['connecting', 'reconnecting', 'resynchronizing'].includes(transport) ? 'connecting'
    : transport === 'connected' ? 'connected' : 'disconnected';
  const previous = useRef(phase), shownAt = useRef<number | undefined>(undefined);
  const [recovered, setRecovered] = useState(false), [holdConnecting, setHoldConnecting] = useState(false);
  useLayoutEffect(() => {
    const before = previous.current; previous.current = phase;
    setRecovered(phase === 'connected' && before !== 'connected');
  }, [phase]);
  useLayoutEffect(() => {
    if (!recovered || holdConnecting) return;
    const timeout = setTimeout(() => setRecovered(false), 2000);
    return () => clearTimeout(timeout);
  }, [recovered, holdConnecting]);
  useLayoutEffect(() => {
    if (phase === 'connecting') { shownAt.current = Date.now(); return; }
    const start = shownAt.current; shownAt.current = undefined;
    if (start === undefined) return;
    const remaining = 800 - (Date.now() - start);
    if (remaining <= 0) return;
    setHoldConnecting(true);
    const timeout = setTimeout(() => setHoldConnecting(false), remaining);
    return () => { clearTimeout(timeout); setHoldConnecting(false); };
  }, [phase]);
  const incompatible = transport === 'incompatible';
  return <><ConnectionIndicator compact={!wide}
    state={phase === 'connecting' || holdConnecting ? 'connecting' : phase === 'disconnected' ? 'disconnected' : recovered ? 'recovered' : undefined}
    disconnectedLabel={tx(incompatible ? 'common:connection.version-mismatch' : 'common:connection.disconnected')}
    connectingLabel={tx('common:connection.connecting')} recoveredLabel={tx('common:connection.recovered')}
    reconnectActionLabel={tx(incompatible ? 'common:session-product.connection-settings' : 'common:connection.reconnect')}
    restartActionLabel={tx('common:connection.restart')}
    onReconnect={incompatible ? settings : () => void connection.reconnect()}/>
    {phase === 'disconnected' && !incompatible && <Tooltip label={tx('common:app.show-details')} side="top"><button type="button" className={css.details} aria-label={tx('common:app.show-details')} onClick={settings}><IconInfoOutline14/></button></Tooltip>}
  </>;
}
