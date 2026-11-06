import { useEffect, useState, useSyncExternalStore } from 'react';
import type { AppServerClient } from '../../client/app-server';
import type { BtwPhase } from '../../client/auxiliary';
import { useTranslation } from '../../locale/react';
import type { TranslationKey } from '../../locale/translation';
import { Button } from '../../presentation/primitives/Button';
import { Modal } from '../../presentation/primitives/Modal';
import css from './BtwPanel.module.css';
const states = {
  submitting: 'agent:btw.submitting', uncertain: 'agent:btw.uncertain', preparing: 'agent:btw.preparing', running: 'agent:btw.running', retrying: 'agent:btw.retrying', settling: 'agent:btw.settling', complete: 'agent:btw.complete', cancelled: 'agent:btw.cancelled', failed: 'agent:btw.failed', incomplete: 'agent:btw.incomplete', unavailable: 'agent:btw.unavailable', notAdmitted: 'agent:btw.notAdmitted',
} as const satisfies Record<BtwPhase, TranslationKey>;
const failures: Record<string, TranslationKey> = {
  invalid_output: 'agent:btw.invalidOutput', refused: 'agent:btw.refused',
  output_too_large: 'agent:btw.outputTooLarge', input_too_large: 'agent:btw.inputTooLarge',
  execution_failed: 'agent:btw.executionFailed', cancelled: 'agent:btw.cancelled', unavailable: 'agent:btw.unavailable',
};
export function BtwPanel({ client, sessionId }: { client: AppServerClient; sessionId: string }) {
  const tx = useTranslation(), [open, setOpen] = useState(false), [draft, setDraft] = useState(''), [error, setError] = useState<string>();
  const question = useSyncExternalStore(client.auxiliary.subscribe, () => client.auxiliary.snapshot(sessionId));
  const available = client.hasAuxiliaryAuthority(sessionId);
  useEffect(() => { if (open && available) return client.auxiliary.observe(sessionId); }, [client, sessionId, open, available]);
  const active = question && !question.settled;
  const run = (work: () => Promise<void>) => { setError(undefined); void work().catch(cause => setError(String(cause).slice(0, 512))); };
  const submit = () => { if (!active && available) run(() => client.auxiliary.submit(sessionId, draft)); };
  if (!client.getSnapshot().capabilities?.delivery_access && !question) return null;
  return <>
    <Button size="sm" disabled={!available} onClick={() => setOpen(true)}>{tx('agent:btw.title')}</Button>
    <Modal open={open} title={tx('agent:btw.title')} closeLabel={tx('agent:btw.close')} description={tx('agent:btw.context')} onClose={() => setOpen(false)}>
      <form className={css.panel} onSubmit={event => { event.preventDefault(); submit(); }}>
        <label>{tx('agent:btw.question')}<textarea value={active ? question.question : draft} disabled={!!active} maxLength={16_384} rows={3} onChange={event => setDraft(event.target.value)} onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing) { event.preventDefault(); submit(); } }}/></label>
        <p>{tx('agent:btw.hint')}</p>
        <div className={css.actions}><Button type="submit" disabled={!available || !!active || !draft || new TextEncoder().encode(draft).length > 16_384}>{tx('agent:btw.submit')}</Button>
          {question && <Button type="button" disabled={!available || question.settled} onClick={() => run(() => client.auxiliary.reconcile(sessionId))}>{tx('agent:btw.check')}</Button>}
          {active && (question.receipt || question.pendingCall) && <Button type="button" disabled={!available || question.cancelling} onClick={() => run(() => client.auxiliary.cancel(sessionId))}>{tx('agent:btw.cancel')}</Button>}
        </div>
        {question && <><p role="status" aria-live="polite">{tx(states[question.phase])}</p><div className={css.answer} data-btw-complete={question.complete}>{question.text}</div>{question.diagnostic && question.diagnostic !== 'cancelled' && <p role="alert">{failures[question.diagnostic] ? tx(failures[question.diagnostic]) : question.diagnostic}</p>}</>}
        {error && <p role="alert">{error}</p>}
      </form>
    </Modal>
  </>;
}
