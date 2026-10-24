import { useEffect, useRef, useState } from 'react';
import type { ScheduleHeader, ScheduleSpec, ScheduledWorkflowRecovery } from '../../../../protocol/app-server/v57';
import type { AppServerClient } from '../../client/app-server';
import { useClientSelector, sameValue } from '../../client/selectors';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { DialogSurface } from '../../presentation/primitives/DialogSurface';
import css from './Schedules.module.css';

interface Scope { session_id: string; node_id: string; generation: number; authorityId?: string }
interface Detail { loading?: boolean; text?: string; error?: string; recoveryLoading?:boolean; recovery?:ScheduledWorkflowRecovery|null; recoveryError?:string }

/** Reads exact native nodes without attaching, switching focus or activating work. */
export function Schedules({ client, sessionId, nodeId, disabled }: { client: AppServerClient; sessionId: string; nodeId: string; disabled?: boolean }) {
  const tx = useTranslation();
  const connection = useClientSelector(client, state => ({ generation: state.generation, authorityId: state.authorityId }), sameValue);
  const [scope, setScope] = useState<Scope>();
  const [rows, setRows] = useState<ScheduleHeader[]>([]);
  const [next, setNext] = useState<string | null>();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const [details, setDetails] = useState<Record<string, Detail>>({});
  const page = useRef({});
  const detailOwners=useRef(new Map<string,object>());
  const current = (held: Scope) => {
    const now = client.getSnapshot();
    return held.generation === now.generation && held.authorityId === now.authorityId;
  };
  const valid = scope && scope.generation === connection.generation && scope.authorityId === connection.authorityId;
  const load = (held: Scope, after: string) => {
    const owner = {};
    page.current = owner;
    const active = () => page.current === owner && current(held);
    setRows([]); setNext(undefined); setDetails({}); setError(undefined); setLoading(true);
    void client.request({ method: 'schedule/list', params: { session_id: held.session_id, node_id: held.node_id, after_ordinal: after, limit: 20 } }, 'schedules', undefined, active)
      .then(result => { if (active()) { setRows(result.schedules); setNext(result.next_ordinal); } })
      .catch(cause => { if (active()) setError(String(cause)); })
      .finally(() => { if (active()) setLoading(false); });
  };
  useEffect(() => {
    if (scope) load(scope, '0');
    return () => { page.current = {}; };
  }, [client, scope]);
  const read = (row: ScheduleHeader) => {
    if (!scope || !valid) return;
    const held = scope, owner = page.current, key = row.id.ordinal, requestOwner={};
    detailOwners.current.set(`body:${key}`,requestOwner);
    const active = () => page.current === owner && current(held) && detailOwners.current.get(`body:${key}`)===requestOwner;
    setDetails(previous => ({ ...previous, [key]: { ...previous[key], loading: true,error:undefined } }));
    void client.request({ method: 'schedule/get', params: { session_id: held.session_id, node_id: held.node_id, id: row.id } }, 'schedule', undefined, active)
      .then(result => { if (active()) setDetails(previous => ({ ...previous, [key]: { ...previous[key], loading:false, ...(result.schedule ? { text: result.schedule.target.type === 'conversation_inbound' ? result.schedule.target.text : JSON.stringify(result.schedule.target.arguments,null,2) } : { error: tx('agent:scheduler.unavailable') }) } })); })
      .catch(cause => { if (active()) setDetails(previous => ({ ...previous, [key]: { ...previous[key],loading:false,error: String(cause) } })); });
  };
  const recovery = (row:ScheduleHeader, after='0') => {
    if (!scope || !valid || row.last_trigger?.target.type!=='workflow') return;
    const held=scope,owner=page.current,key=row.id.ordinal,requestOwner={};
    detailOwners.current.set(`recovery:${key}`,requestOwner);
    const active=()=>page.current===owner && current(held) && detailOwners.current.get(`recovery:${key}`)===requestOwner;
    setDetails(previous=>({...previous,[key]:{...previous[key],recoveryLoading:true,recoveryError:undefined}}));
    void client.request({method:'workflow/scheduled/getRecovery',params:{session_id:held.session_id,node_id:held.node_id,start_id:row.last_trigger.target.start_id,after_effect_ordinal:after,limit:20}},'scheduled_workflow_recovery',undefined,active)
      .then(result=>{if(active())setDetails(previous=>({...previous,[key]:{...previous[key],recoveryLoading:false,recovery:result.recovery}}));})
      .catch(cause=>{if(active())setDetails(previous=>({...previous,[key]:{...previous[key],recoveryLoading:false,recoveryError:String(cause)}}));});
  };
  const execution = (value:ScheduledWorkflowRecovery) => {
    if(value.start.state.type==='accepted')return tx('agent:scheduler.execution.waiting');
    if(value.start.state.type==='claimed')return tx(value.start.recovery_blocked?'agent:scheduler.execution.blocked':'agent:scheduler.execution.claimed');
    switch(value.start.state.outcome.type){
      case 'completed':return tx('agent:scheduler.execution.completed');
      case 'cancelled':return tx('agent:scheduler.execution.cancelled');
      case 'timed_out':return tx('agent:scheduler.execution.timed-out');
      case 'rejected_before_start':return tx('agent:scheduler.execution.rejected');
      case 'interrupted':return tx('agent:scheduler.execution.interrupted');
      case 'outcome_unknown':return tx('agent:scheduler.execution.unknown');
      case 'failed':return tx('agent:scheduler.execution.failed');
    }
  };
  const rule = (spec: ScheduleSpec) => {
    if (spec.type === 'at') return tx('agent:scheduler.at', { time: spec.at_utc });
    if (spec.type === 'interval') return tx('agent:scheduler.interval', { seconds: spec.every_seconds, anchor: spec.anchor_utc });
    return `${spec.expression} (${spec.time_zone})`;
  };
  const close = () => { page.current = {}; setScope(undefined); };
  return <>
    <Button size="sm" disabled={disabled} onClick={() => { setRows([]); setDetails({}); setLoading(true); setScope({ session_id: sessionId, node_id: nodeId, ...connection }); }}>{tx('agent:scheduler.title')}</Button>
    <DialogSurface open={!!scope} onClose={close} title={tx('agent:scheduler.title')} overlayClassName={css.overlay} className={css.dialog}>
      <div className={css.heading}><h2>{tx('agent:scheduler.title')}</h2><Button onClick={close}>{tx('agent:scheduler.close')}</Button></div>
      {scope && <p className={css.scope}>{tx('agent:scheduler.target', { session: scope.session_id, node: scope.node_id })}</p>}
      <p>{tx('agent:scheduler.admission')}</p>
      {!valid && scope && <p role="alert">{tx('agent:scheduler.stale')}</p>}
      <div className={css.actions}><Button disabled={!valid || loading} onClick={() => scope && load(scope, '0')}>{tx('agent:scheduler.refresh')}</Button>
        <Button disabled={!valid || loading || next == null} onClick={() => scope && next != null && load(scope, next)}>{tx('agent:scheduler.next')}</Button></div>
      {loading && valid && <p role="status">{tx('agent:scheduler.loading')}</p>}
      {error && <p role="alert">{error}</p>}
      {!loading && valid && rows.length === 0 && !error && <p>{tx('agent:scheduler.empty')}</p>}
      <ol className={css.list}>{rows.map(row => <li key={row.id.ordinal}>
        <div className={css.heading}><strong>{row.title}</strong><span>{tx(row.state === 'enabled' ? 'agent:scheduler.enabled' : row.state === 'disabled' ? 'agent:scheduler.disabled' : 'agent:scheduler.completed')}</span></div>
        <p>{rule(row.spec)}</p>
        {row.target.type==='workflow' && <p>{tx('agent:scheduler.workflow',{id:row.target.workflow_id})}</p>}
        {row.next_due_utc && <p>{tx('agent:scheduler.due', { time: row.next_due_utc })}</p>}
        {row.last_trigger && <p>{tx('agent:scheduler.accepted', { time: row.last_trigger.accepted_at })}</p>}
        {row.last_failure && <p role="alert">{row.last_failure}</p>}
        <Button size="sm" disabled={!valid || details[row.id.ordinal]?.loading} onClick={() => read(row)}>{tx(row.target.type==='workflow'?'agent:scheduler.arguments':'agent:scheduler.message')}</Button>
        {details[row.id.ordinal]?.text !== undefined && <p className={css.message}>{details[row.id.ordinal].text}</p>}
        {details[row.id.ordinal]?.error && <p role="alert">{details[row.id.ordinal].error}</p>}
        {row.last_trigger?.target.type==='workflow' && <Button size="sm" disabled={!valid || details[row.id.ordinal]?.recoveryLoading} onClick={()=>recovery(row)}>{tx('agent:scheduler.recovery')}</Button>}
        {details[row.id.ordinal]?.recoveryLoading && <p role="status">{tx('agent:scheduler.loading')}</p>}
        {details[row.id.ordinal]?.recoveryError && <p role="alert">{details[row.id.ordinal].recoveryError}</p>}
        {details[row.id.ordinal]?.recovery && <div className={css.recovery}>
          <p>{execution(details[row.id.ordinal].recovery!)}</p>
          {(() => {const state=details[row.id.ordinal].recovery!.start.state;const reason=state.type==='accepted'?state.waiting:state.type==='settled' && 'reason' in state.outcome?state.outcome.reason:undefined;return reason && <p>{reason}</p>;})()}
          {details[row.id.ordinal].recovery!.start.cancellation_requested_at && <p>{tx('agent:scheduler.cancellation')}</p>}
          {details[row.id.ordinal].recovery!.capacity_blocked && <p>{tx('agent:scheduler.ownership')}</p>}
          {details[row.id.ordinal].recovery!.start.recovery_blocked && <p role="alert">{details[row.id.ordinal].recovery!.start.recovery_blocked}</p>}
          <ul>{details[row.id.ordinal].recovery!.effects.map(effect=><li key={effect.ordinal}>
            <strong>{effect.node.node}</strong> · {tx(effect.physical_settlement_proven?'agent:scheduler.proven':'agent:scheduler.unproven')}
            {effect.owner.type==='agent' && <p className={css.scope}>{effect.owner.activation}</p>}
            {effect.owner.type==='model' && <p className={css.scope}>{effect.owner.call_id}</p>}
            {effect.diagnostic && <p>{effect.diagnostic}</p>}
          </li>)}</ul>
          {details[row.id.ordinal].recovery!.result!=null && <><p>{tx('agent:scheduler.result')}</p><pre className={css.message}>{JSON.stringify(details[row.id.ordinal].recovery!.result,null,2)}</pre></>}
          {details[row.id.ordinal].recovery!.next_effect_ordinal!=null && <Button size="sm" disabled={!valid || details[row.id.ordinal]?.recoveryLoading} onClick={()=>recovery(row,details[row.id.ordinal].recovery!.next_effect_ordinal!)}>{tx('agent:scheduler.next-effects')}</Button>}
        </div>}

      </li>)}</ol>
    </DialogSurface>
  </>;
}
