import { useTranslation } from '../locale/react';
import { useSelector } from '@xstate/react';
import { useState, type ReactNode } from 'react';
import type { SourceTarget } from '../../../protocol/app-server/v80';
import type { AppServerClient } from '../client/app-server';
import type { SessionView, ConnectionState } from '../client/contracts';
import { Button } from '../presentation/primitives/Button';
import { StateDot, type StateDotState } from '../presentation/primitives/StateDot';
import { useSessionConfiguration, type SessionConfigurationActor } from './settings/machines/react';
import { applicationOwners, observedResult, observedUnitLabel, observedUnits, openOwnerLabel, sourceTargetKey, unitApplication } from './settings/projection';

/** The Session-owned configuration status line.
 *
 * Native prepares a saved configuration and commits it automatically at the
 * Session's own safe boundary; nothing here confirms or schedules anything.
 * The line shows only facts that need attention — a
 * prepared change waiting for current work to settle, a failure that keeps the
 * previous configuration, an uncertain commit, or settings that wait for a
 * restart — and disappears once everything is applied. Success is silent.
 *
 * This component issues no read. Native publications and reconnects are
 * transport facts the actor observes from its configuration system, so a
 * Session observation recovers after a reconnect whether or not this
 * presentation renders. */
export function SessionConfiguration({ client, view, openOwningSettings }: { client: AppServerClient; view: SessionView; openOwningSettings?: (owner: SourceTarget) => void }) {
  const { actor, transport } = useSessionConfiguration(client, view.id);
  return actor ? <ConfigurationObservation actor={actor} client={client} connection={transport.connection} openOwningSettings={openOwningSettings}/> : null;
}
function ConfigurationObservation({ actor, client, connection, openOwningSettings }: {
  actor: SessionConfigurationActor; client: AppServerClient; connection: ConnectionState; openOwningSettings?: (owner: SourceTarget) => void;
}) {
  const tx = useTranslation();
  // Retain actionable configuration details while a reconnect read is in flight.
  const application = useSelector(actor, snapshot => snapshot.context.application ?? snapshot.context.staleApplication);
  const readError = useSelector(actor, snapshot => snapshot.context.readError);
  const [retrying, setRetrying] = useState(false);
  // A physical outage is already actionable beside Settings. Keep explicit
  // configuration facts without repeating that outage.
  const unavailable = connection === 'connected' && !!readError;
  // Per-unit native observations. Independent units may simultaneously be
  // preparing, waiting, failed or already applied; none of them is flattened
  // into one global success state.
  const observations = observedUnits.map(unit => ({ unit, result: observedResult(unitApplication(application, unit)) })).filter(row => row.result.state !== 'unavailable');
  const of = (state: string) => observations.filter(row => row.result.state === state);
  const waiting = of('ready'), failed = of('failed'), uncertain = of('uncertain'), restart = of('restart_pending');
  if (!unavailable && !waiting.length && !failed.length && !uncertain.length && !restart.length && !readError) return null;
  const owners = applicationOwners(application);
  // Retry is a native rescan of the owning sources; it never replays a save.
  const retry = async () => {
    setRetrying(true);
    try {
      for (const target of owners) await client.request({ method: 'configuration/reconcile', params: { target } }, 'configuration_receipt');
    } finally { setRetrying(false); }
  };
  const detail = (rows: typeof observations) => rows.map(row => row.result.state === 'failed' || row.result.state === 'uncertain'
    ? tx('common:copy.value-failed-value', { p0: observedUnitLabel(tx, row.unit), p1: row.result.diagnostic })
    : observedUnitLabel(tx, row.unit));
  return <section aria-label={tx('common:session-configuration.session-configuration')} className="flex flex-col gap-[6px] min-w-0 mt-[8px] mx-0 mb-0 [color:var(--dsw-alias-label-primary)] text-[13px] leading-[20px] [container:session-configuration/inline-size]">
    {unavailable && <Line state="unavailable" text={tx('common:copy.configuration-status-unavailable-retaining-the-last-observation')} />}
    {waiting.length > 0 && <Line state="waiting" text={tx('common:copy.configuration-applies-after-current-work')} detail={detail(waiting)} />}
    {failed.length > 0 && <Line state="failed" text={tx('common:copy.configuration-failed-previous-kept')} detail={detail(failed)}
      // Native names the authored owners of this application; `scope` is the
      // Session identity and is never one of them.
      actions={<>
        {openOwningSettings && owners.map(owner =>
          <Button size="sm" variant="outline" key={sourceTargetKey(owner)} onClick={() => openOwningSettings(owner)}>{openOwnerLabel(tx, owner)}</Button>)}
        {owners.length > 0 && <Button size="sm" variant="outline" disabled={retrying || connection !== 'connected'} onClick={() => void retry()}>{tx('common:session-configuration.retry')}</Button>}
      </>} />}
    {uncertain.length > 0 && <Line state="failed" text={tx('common:copy.configuration-uncertain')} detail={detail(uncertain)} />}
    {restart.length > 0 && <Line state="waiting" text={tx('common:copy.configuration-restart-required')} detail={detail(restart)} />}
    {readError && <p role="alert" className="m-0 px-[12px] py-[6px] rounded-[12px] [color:var(--dsw-alias-label-primary)] bg-[color-mix(in_srgb,var(--dsw-alias-state-error-primary)_8%,transparent)] [border-left:3px_solid_var(--dsw-alias-state-error-primary)] wrap-anywhere">{readError}</p>}
  </section>;
}

type LineState = 'unavailable' | 'waiting' | 'failed';
const dots: Record<LineState, StateDotState> = { unavailable: 'idle', waiting: 'warning', failed: /* i18n-raw: internal severity identity */ 'error' };

/** One native fact of the banner, on one compact horizontal line: its state,
 * its text, the per-unit detail, and the action that belongs to exactly this
 * fact. The state is always written out; the dot only repeats it. On a narrow
 * Session the line wraps instead of truncating. */
function Line({ state, text, detail, actions }: {
  state: LineState; text: string; detail?: readonly string[]; actions?: ReactNode;
}) {
  return <div className="grid grid-cols-[auto_minmax(0,1fr)_auto] [grid-template-areas:'dot_text_actions'] items-center gap-x-[10px] gap-y-[6px] min-w-0 min-h-[40px] box-border pt-[6px] pr-[8px] pb-[6px] pl-[12px] [border:0.5px_solid_var(--dsw-alias-border-l2)] rounded-[12px] bg-[var(--dsw-alias-bg-module-platform)] data-[state=failed]:[border-color:color-mix(in_srgb,var(--dsw-alias-state-error-primary)_40%,transparent)] [@container_session-configuration_(max-width:560px)]:grid-cols-[auto_minmax(0,1fr)] [@container_session-configuration_(max-width:560px)]:[grid-template-areas:'dot_text'_'._actions'] [@container_session-configuration_(max-width:560px)]:pr-[10px]" data-state={state}>
    <StateDot state={dots[state]} className="[grid-area:dot] self-start mt-[5px]" />
    <div className="[grid-area:text] min-w-0 wrap-anywhere [&_p]:m-0">
      <p role="status">{text}</p>
      {!!detail?.length && <ul className="mt-[2px] mx-0 mb-0 p-0 list-none [color:var(--dsw-alias-label-secondary)] text-[12px] leading-[18px]">{detail.map(item => <li key={item}>{item}</li>)}</ul>}
    </div>
    {actions && <div className="[grid-area:actions] flex flex-wrap justify-end gap-[6px] min-w-0 [@container_session-configuration_(max-width:560px)]:justify-start [&_button]:max-w-full [&_button]:whitespace-normal [&_button]:text-left [&_button]:h-auto [&_button]:min-h-[28px]">{actions}</div>}
  </div>;
}
