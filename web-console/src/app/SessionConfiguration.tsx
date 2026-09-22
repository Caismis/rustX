import { useEffect } from 'react';
import { useSelector } from '@xstate/react';
import type { SourceTarget } from '../../../protocol/app-server/v18';
import type { AppServerClient, SessionView } from '../client/app-server';
import { Button } from '../presentation/primitives/Button';
import { useSessionConfiguration } from './settings/machines/react';
import { applicationOwners, observedResult, observedUnitLabel, observedUnits, openOwnerLabel, sourceTargetKey, unitApplication } from './settings/projection';

/** The Session-owned configuration region.
 *
 * Observation and adoption are two independent facts, owned by two regions of
 * one actor. A successful authoritative read clears the read failure it answers
 * and nothing else, and an adoption response clears no read failure, because
 * neither region can write the other's field. Mutation acknowledgements never
 * clear pending observations, and this component is never the adoption gate:
 * `session/adoptConfiguration` revalidates it natively. */
export function SessionConfiguration({ client, view, openOwningSettings }: { client: AppServerClient; view: SessionView; openOwningSettings?: (owner: SourceTarget) => void }) {
  const { actor, transport } = useSessionConfiguration(client, view.id);
  // The current generation's observation, or — while a replaced generation's
  // successor has not observed yet — the previous one as explicitly stale
  // presentation data. `known` is what says which of the two this is; the
  // retained value is never a comparison baseline for the new generation.
  const application = useSelector(actor, snapshot => snapshot.context.application ?? snapshot.context.staleApplication);
  const readError = useSelector(actor, snapshot => snapshot.context.readError);
  const adoptionError = useSelector(actor, snapshot => snapshot.context.adoptionError);
  const known = useSelector(actor, snapshot => snapshot.matches({ observation: 'ready' }));
  const busy = useSelector(actor, snapshot => snapshot.matches({ adoption: 'submitting' }));
  // Native publication and Session snapshot changes are observation triggers,
  // never adoption triggers. The actor owns read ordering: a refresh supersedes
  // the read in flight by stopping it.
  const version = transport.configuration?.[view.id]?.version;
  const snapshot = view.snapshot;
  useEffect(() => { actor.send({ type: 'REFRESH' }); }, [actor, version, snapshot]);
  const candidate = application?.candidate;
  // Per-unit native observations. Independent units may simultaneously be
  // preparing, failed or already applied; none of them is flattened into one
  // global success state or into Session adoption.
  const observations = observedUnits.map(unit => ({ unit, result: observedResult(unitApplication(application, unit)) })).filter(row => row.result.state !== 'unavailable');
  const preparing = observations.filter(row => row.result.state === 'preparing');
  const failed = observations.filter(row => row.result.state === 'failed');
  if (known && !candidate && !preparing.length && !failed.length && !adoptionError) return null;
  return <section aria-label="Session configuration" role="status">
    {!known && <p>Configuration status unavailable. Retaining the last observation.</p>}
    {preparing.length > 0 && <><p>Preparing configuration…</p><ul>{preparing.map(row => <li key={row.unit}>{observedUnitLabel(row.unit)}: preparing</li>)}</ul></>}
    {candidate && <><p>Prepared configuration is waiting for this Session.</p>
      {application?.eligibility.status === 'busy' && <p>Session work must settle before adoption.</p>}
      {application?.eligibility.status === 'unavailable' && <p>Session configuration is unavailable for adoption.</p>}
      <Button disabled={!known || busy || transport.connection !== 'connected' || application?.eligibility.status !== 'eligible'}
        onClick={() => actor.send({ type: 'ADOPT', candidate })}>Adopt configuration</Button></>}
    {failed.length > 0 && <><p>Some configuration preparation failed. Review the owning authored source in Settings and rescan.</p>
      <ul>{failed.map(row => <li key={row.unit}>{observedUnitLabel(row.unit)}: failed — {row.result.state === 'failed' ? row.result.diagnostic : ''}</li>)}</ul>
      {/* Native names the authored owners of this application; `scope` is the
          Session identity and is never one of them. Each owner is offered
          explicitly, so no ownership is parsed, guessed or defaulted here. */}
      {openOwningSettings && applicationOwners(application).map(owner =>
        <Button key={sourceTargetKey(owner)} onClick={() => openOwningSettings(owner)}>{openOwnerLabel(owner)}</Button>)}</>}
    {/* Read failure and adoption failure are separate facts, reported
        separately; neither one clears or hides the other. */}
    {readError && <p role="alert">{readError}</p>}
    {adoptionError && <p role="alert">{adoptionError}</p>}
  </section>;
}
