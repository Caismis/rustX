import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { ConfigurationApplication } from '../../../protocol/app-server/v17';
import { isOutcomeUncertain, type AppServerClient, type SessionView } from '../client/app-server';
import { Button } from '../presentation/primitives/Button';

/** Mutation acknowledgements never clear pending observations. */
export function SessionConfiguration({ client, view }: { client: AppServerClient; view: SessionView }) {
  const transport = useSyncExternalStore(client.subscribe, client.getSnapshot);
  const [application, setApplication] = useState<ConfigurationApplication>();
  const [known, setKnown] = useState(false), [error, setError] = useState(''), [busy, setBusy] = useState(false);
  const epoch = useRef(0), reads = useRef(0), submitting = useRef<number | undefined>(undefined);
  const refresh = useCallback(async () => {
    const at = epoch.current, sequence = ++reads.current;
    try {
      const result = await client.request({ method: 'session/configuration', params: { session_id: view.id } }, 'session_configuration');
      if (at !== epoch.current || sequence !== reads.current) return;
      setApplication(previous => previous && result.application && BigInt(previous.version) > BigInt(result.application.version) ? previous : result.application ?? undefined);
      setKnown(true);
    } catch (cause) { if (at === epoch.current && sequence === reads.current) { setKnown(false); setError(String(cause)); } }
  }, [client, view.id]);
  useEffect(() => { ++epoch.current; setKnown(false); setBusy(false); return () => { ++epoch.current; }; }, [refresh, transport.generation]);
  const version = transport.configuration?.[view.id]?.version;
  useEffect(() => { void refresh(); }, [refresh, version, view.snapshot, transport.generation]);
  const candidate = application?.candidate;
  const adopt = async () => {
    if (!candidate || !known || application?.eligibility.status !== 'eligible' || submitting.current === epoch.current) return;
    const at = epoch.current;
    submitting.current = at; setBusy(true); setError('');
    try {
      await client.request({ method: 'session/adoptConfiguration', params: { session_id: view.id, candidate: candidate.identity, expected_binding: candidate.expected_binding } }, 'configuration_application');
    } catch (cause) {
      if (at === epoch.current) setError(isOutcomeUncertain(cause) ? 'Adoption outcome uncertain. Rereading authority; adoption will not be replayed.' : String(cause));
    } finally {
      if (at === epoch.current) { await refresh(); setBusy(false); }
      if (submitting.current === at) submitting.current = undefined;
    }
  };
  const units = application ? [application.units.capabilities, application.units.instructions, application.units.provider] : [];
  const preparing = units.some(unit => unit?.status === 'preparing'), failed = units.some(unit => unit?.status === 'failed');
  if (known && !candidate && !preparing && !failed) return null;
  return <section aria-label="Session configuration" role="status">
    {!known && <p>Configuration status unavailable. Retaining the last observation.</p>}
    {preparing && <p>Preparing configuration…</p>}
    {candidate && <><p>Prepared configuration is waiting for this Session.</p>
      {application?.eligibility.status === 'busy' && <p>Session work must settle before adoption.</p>}
      {application?.eligibility.status === 'unavailable' && <p>Session configuration is unavailable for adoption.</p>}
      <Button disabled={!known || busy || transport.connection !== 'connected' || application?.eligibility.status !== 'eligible'} onClick={() => void adopt()}>Adopt configuration</Button></>}
    {failed && <p>Some configuration preparation failed. Review the owning User or Workspace source in Settings and rescan.</p>}
    {error && <p role="alert">{error}</p>}
  </section>;
}
