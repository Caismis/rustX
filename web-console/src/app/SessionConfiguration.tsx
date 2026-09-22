import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import type { ConfigurationApplication, SourceTarget } from '../../../protocol/app-server/v18';
import { isOutcomeUncertain, type AppServerClient, type SessionView } from '../client/app-server';
import { Button } from '../presentation/primitives/Button';
import { applicationOwners, observedResult, observedUnitLabel, observedUnits, openOwnerLabel, sourceTargetKey, unitApplication } from './settings/projection';

/** Mutation acknowledgements never clear pending observations. */
export function SessionConfiguration({ client, view, openOwningSettings }: { client: AppServerClient; view: SessionView; openOwningSettings?: (owner: SourceTarget) => void }) {
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
      // Terminal client-side cleanup owns nothing but this attempt's own state,
      // and it releases that state before anything that can fail. The
      // authoritative reread is a separate obligation: however it settles, it
      // can neither strand the in-flight guard nor leave `busy` latched, and it
      // never replays the adoption. A failed reread therefore leaves exactly
      // "uncertain and visible", and a later native observation can make the
      // candidate actionable again.
      if (submitting.current === at) submitting.current = undefined;
      if (at === epoch.current) {
        try { await refresh(); } finally { if (at === epoch.current) setBusy(false); }
      }
    }
  };
  // Per-unit native observations. Independent units may simultaneously be
  // preparing, failed or already applied; none of them is flattened into one
  // global success state or into Session adoption.
  const observations = observedUnits.map(unit => ({ unit, result: observedResult(unitApplication(application, unit)) })).filter(row => row.result.state !== 'unavailable');
  const preparing = observations.filter(row => row.result.state === 'preparing');
  const failed = observations.filter(row => row.result.state === 'failed');
  if (known && !candidate && !preparing.length && !failed.length) return null;
  return <section aria-label="Session configuration" role="status">
    {!known && <p>Configuration status unavailable. Retaining the last observation.</p>}
    {preparing.length > 0 && <><p>Preparing configuration…</p><ul>{preparing.map(row => <li key={row.unit}>{observedUnitLabel(row.unit)}: preparing</li>)}</ul></>}
    {candidate && <><p>Prepared configuration is waiting for this Session.</p>
      {application?.eligibility.status === 'busy' && <p>Session work must settle before adoption.</p>}
      {application?.eligibility.status === 'unavailable' && <p>Session configuration is unavailable for adoption.</p>}
      <Button disabled={!known || busy || transport.connection !== 'connected' || application?.eligibility.status !== 'eligible'} onClick={() => void adopt()}>Adopt configuration</Button></>}
    {failed.length > 0 && <><p>Some configuration preparation failed. Review the owning authored source in Settings and rescan.</p>
      <ul>{failed.map(row => <li key={row.unit}>{observedUnitLabel(row.unit)}: failed — {row.result.state === 'failed' ? row.result.diagnostic : ''}</li>)}</ul>
      {/* Native names the authored owners of this application; `scope` is the
          Session identity and is never one of them. Each owner is offered
          explicitly, so no ownership is parsed, guessed or defaulted here. */}
      {openOwningSettings && applicationOwners(application).map(owner =>
        <Button key={sourceTargetKey(owner)} onClick={() => openOwningSettings(owner)}>{openOwnerLabel(owner)}</Button>)}</>}
    {error && <p role="alert">{error}</p>}
  </section>;
}
