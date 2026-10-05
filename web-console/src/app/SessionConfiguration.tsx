import { useTranslation } from '../locale/react';
import { useSelector } from '@xstate/react';
import { useClientSelector } from '../client/selectors';
import type { AdoptionEligibility, SourceTarget } from '../../../protocol/app-server/v35';
import type { AppServerClient, SessionView, ConnectionState } from '../client/app-server';
import type { ReactNode } from 'react';
import { Button } from '../presentation/primitives/Button';
import { StateDot, type StateDotState } from '../presentation/primitives/StateDot';
import css from '../presentation/agent/SessionConfiguration.module.css';
import { useSessionConfiguration, type SessionConfigurationActor } from './settings/machines/react';
import { applicationCurrent, observationUnavailable } from './settings/machines/session-configuration';
import { applicationOwners, observedResult, observedUnitLabel, observedUnits, openOwnerLabel, sourceTargetKey, unitApplication } from './settings/projection';

/** The Session-owned configuration region.
 *
 * Observation and adoption are two independent facts, owned by two regions of
 * one actor. A successful authoritative read clears the read failure it answers
 * and nothing else, and an adoption response clears no read failure, because
 * neither region can write the other's field. Mutation acknowledgements never
 * clear pending observations, and this component is never the adoption gate:
 * `session/adoptConfiguration` revalidates it natively.
 *
 * This component issues no read. Native publications and reconnects are
 * transport facts the actor observes from its configuration system, so a
 * Session observation recovers after a reconnect whether or not this
 * presentation renders, and whether or not the Session is attached.
 *
 * Adoption eligibility is not configuration state: it is the live runtime's
 * own advisory publication on the Session's Runtime Client snapshot, read here
 * as-is and never derived from attempts, jobs or interactions. A streamed
 * delta changes the snapshot, never the configuration observation. */
export function SessionConfiguration({ client, view, openOwningSettings }: { client: AppServerClient; view: SessionView; openOwningSettings?: (owner: SourceTarget) => void }) {
  const { actor, transport } = useSessionConfiguration(client, view.id);
  // Subscribed to this one runtime fact alone, so a streamed delta re-renders
  // nothing here. Without a live runtime snapshot nothing native vouches for
  // eligibility.
  const eligibility = useClientSelector(client, state => state.views[view.id]?.snapshot?.configuration_adoption_eligibility.status ?? 'unavailable');
  return actor ? <ConfigurationObservation actor={actor} connection={transport.connection} eligibility={eligibility} openOwningSettings={openOwningSettings}/> : null;
}
function ConfigurationObservation({ actor, connection, eligibility, openOwningSettings }: {
  actor: SessionConfigurationActor; connection: ConnectionState; eligibility: AdoptionEligibility['status']; openOwningSettings?: (owner: SourceTarget) => void;
}) {
  const tx = useTranslation();
  // The current connected span's observation, or — while no span has observed
  // since the last one ended — that span's observation as explicitly stale
  // presentation data, which is then labelled unavailable; the retained value
  // is never a comparison baseline for a later span.
  const application = useSelector(actor, snapshot => snapshot.context.application ?? snapshot.context.staleApplication);
  const readError = useSelector(actor, snapshot => snapshot.context.readError);
  const adoptionError = useSelector(actor, snapshot => snapshot.context.adoptionError);
  // Distinct facts, never one boolean: status is *unavailable* only while the
  // transport cannot read, the span's read failed, or only an ended span's
  // observation is retained; a
  // reread in flight is still known, merely not *current*, and only a current
  // observation offers adoption — still subject to native eligibility.
  const unavailable = useSelector(actor, observationUnavailable);
  const current = useSelector(actor, applicationCurrent);
  const busy = useSelector(actor, snapshot => snapshot.matches({ adoption: 'submitting' }));
  const candidate = application?.candidate;
  // Per-unit native observations. Independent units may simultaneously be
  // preparing, failed or already applied; none of them is flattened into one
  // global success state or into Session adoption.
  const observations = observedUnits.map(unit => ({ unit, result: observedResult(unitApplication(application, unit)) })).filter(row => row.result.state !== 'unavailable');
  const preparing = observations.filter(row => row.result.state === 'preparing');
  const failed = observations.filter(row => row.result.state === 'failed');
  if (!unavailable && !candidate && !preparing.length && !failed.length && !adoptionError) return null;
  // Presentation only: which line of the banner each native fact is. Nothing
  // here decides eligibility, adoption, ownership or residency.
  const owners = openOwningSettings ? applicationOwners(application) : [];
  return <section aria-label={tx('common:session-configuration.session-configuration')} className={css.banner}>
    {unavailable && <Line state="unavailable" text={tx('common:copy.configuration-status-unavailable-retaining-the-last-observation')} />}
    {preparing.length > 0 && <Line state="preparing" text={tx('common:copy.preparing-configuration')}
      detail={preparing.map(row => tx('common:copy.value-preparing', { p0: observedUnitLabel(tx, row.unit) }))} />}
    {candidate && <Line state={eligibility === 'eligible' ? 'ready' : 'blocked'} text={tx('common:copy.prepared-configuration-is-waiting-for-this-session')}
      reason={eligibility === 'busy' ? tx('common:copy.session-work-must-settle-before-adoption')
        : eligibility === 'unavailable' ? tx('common:copy.session-configuration-is-unavailable-for-adoption') : undefined}
      actions={<Button size="sm" variant="primary" disabled={!current || busy || connection !== 'connected' || eligibility !== 'eligible'}
        onClick={() => actor.send({ type: 'ADOPT', candidate })}>{tx('common:session-configuration.adopt-configuration')}</Button>} />}
    {failed.length > 0 && <Line state="failed" text={tx('common:copy.some-configuration-preparation-failed-review-the-owning-authored-source-in-settings-and-re')}
      detail={failed.map(row => tx('common:copy.value-failed-value', { p0: observedUnitLabel(tx, row.unit), p1: row.result.state === 'failed' ? row.result.diagnostic : '' }))}
      // Native names the authored owners of this application; `scope` is the
      // Session identity and is never one of them. Each owner is offered
      // explicitly, so no ownership is parsed, guessed or defaulted here.
      actions={owners.length > 0 && owners.map(owner =>
        <Button size="sm" variant="outline" key={sourceTargetKey(owner)} onClick={() => openOwningSettings!(owner)}>{openOwnerLabel(tx, owner)}</Button>)} />}
    {/* Read failure and adoption failure are separate facts, reported
        separately; neither one clears or hides the other. */}
    {readError && <p role="alert" className={css.alert}>{readError}</p>}
    {adoptionError && <p role="alert" className={css.alert}>{adoptionError}</p>}
  </section>;
}

type LineState = 'unavailable' | 'preparing' | 'ready' | 'blocked' | 'failed';
const dots: Record<LineState, StateDotState> = { unavailable: 'idle', preparing: 'ongoing', ready: 'done', blocked: 'warning', failed: /* i18n-raw: internal severity identity */ 'error' };

/** One native fact of the banner, on one compact horizontal line: its state,
 * its text, the native reason or per-unit detail, and the action that belongs
 * to exactly this fact. The state is always written out; the dot only repeats
 * it. On a narrow Session the line wraps instead of truncating. */
function Line({ state, text, reason, detail, actions }: {
  state: LineState; text: string; reason?: string; detail?: readonly string[]; actions?: ReactNode;
}) {
  return <div className={css.line} data-state={state}>
    <StateDot state={dots[state]} className={css.dot} />
    <div className={css.text}>
      <p role="status">{text}{reason && <> <span className={css.reason}>{reason}</span></>}</p>
      {!!detail?.length && <ul className={css.detail}>{detail.map(item => <li key={item}>{item}</li>)}</ul>}
    </div>
    {actions && <div className={css.actions}>{actions}</div>}
  </div>;
}
