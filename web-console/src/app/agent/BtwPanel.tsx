import { useEffect, useId, useRef, useState, useSyncExternalStore } from 'react';
import type { AppServerClient } from '../../client/app-server';
import type { BtwPhase } from '../../client/auxiliary';
import { useTranslation } from '../../locale/react';
import type { TranslationKey } from '../../locale/translation';
import { Button } from '../../presentation/primitives/Button';
import { IconQuestionOutline14, IconSendOutline14, IconStopFill16 } from '../../presentation/primitives/icons';
import { UserMessage, AssistantMessage } from '../../presentation/agent/Message';
import { MarkdownText } from '../../presentation/markdown/MarkdownText';
import { ChatViewport } from '../../presentation/layout/ChatViewport';
import composerCss from '../../presentation/agent/Composer.module.css';
import css from './BtwPanel.module.css';
const states = {
  submitting: 'agent:btw.submitting', uncertain: 'agent:btw.uncertain', preparing: 'agent:btw.preparing', running: 'agent:btw.running', retrying: 'agent:btw.retrying', settling: 'agent:btw.settling', complete: 'agent:btw.complete', cancelled: 'agent:btw.cancelled', failed: 'agent:btw.failed', incomplete: 'agent:btw.incomplete', unavailable: 'agent:btw.unavailable', notAdmitted: 'agent:btw.notAdmitted',
} as const satisfies Record<BtwPhase, TranslationKey>;
const failures: Record<string, TranslationKey> = {
  invalid_output: 'agent:btw.invalidOutput', refused: 'agent:btw.refused',
  output_too_large: 'agent:btw.outputTooLarge', input_too_large: 'agent:btw.inputTooLarge',
  execution_failed: 'agent:btw.executionFailed', cancelled: 'agent:btw.cancelled', unavailable: 'agent:btw.unavailable',
};
export function BtwPanel({ client, sessionId, visible, close }: { client: AppServerClient; sessionId: string; visible: boolean; close: () => void }) {
  const tx = useTranslation(), [error, setError] = useState<string>();
  const root = useRef<HTMLFormElement>(null), hintId = useId();
  const question = useSyncExternalStore(client.auxiliary.subscribe, () => client.auxiliary.snapshot(sessionId));
  const draft = useSyncExternalStore(client.auxiliary.subscribe, () => client.auxiliary.draft(sessionId));
  const available = useSyncExternalStore(client.subscribe, () => client.hasAuxiliaryAuthority(sessionId));
  useEffect(() => { if (visible && available) return client.auxiliary.observe(sessionId); }, [client, sessionId, visible, available]);
  // The dock removes its ancestor's inert state in a layout effect. Focus only
  // after that visibility handoff has committed, including a collapsed reopen.
  useEffect(() => { if (visible) (root.current?.querySelector<HTMLTextAreaElement>('textarea:not(:disabled)') ?? root.current)?.focus({ preventScroll: true }); }, [visible, sessionId]);
  const active = question && !question.settled;
  const cancellable = !!(active && (question.receipt || question.pendingCall));
  const run = (work: () => Promise<void>) => { setError(undefined); void work().catch(cause => setError(String(cause).slice(0, 512))); };
  const submit = () => { if (!active && available) run(() => client.auxiliary.submit(sessionId, draft)); };
  if (!client.getSnapshot().capabilities?.delivery_access && !question) return null;
  return <form ref={root} className={css.panel} aria-label={tx('agent:btw.title')} tabIndex={-1} onSubmit={event => { event.preventDefault(); submit(); }} onKeyDown={event => {
    if (event.key !== 'Escape' || event.defaultPrevented || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229 || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    event.preventDefault(); event.stopPropagation(); if (!event.repeat) close();
  }}>
      <header className={css.header}>
        <span className={css.badge}><IconQuestionOutline14/>{tx('agent:btw.temporary')}</span>
        <span title={tx('agent:btw.context')}>{tx('agent:btw.snapshot')}</span>
      </header>
      <div className={css.messages} data-btw-transcript>
        <ChatViewport resetKey={question?.correlation ?? sessionId} chromeVisible={visible} latestLabel={tx('agent:agent-transcript.return-to-latest')}>
          {question ? <div className={css.exchange}>
            <div data-chat-anchor-key={`${question.correlation}:question`} data-chat-flow-kind="user"><UserMessage label={tx('agent:message.your-message')}>{question.question}</UserMessage></div>
            {question.text && <div data-chat-anchor-key={`${question.correlation}:answer`}><AssistantMessage label={tx('agent:message.assistant-response')}><div data-btw-complete={question.complete}><MarkdownText text={question.text} streaming={!question.settled}/></div></AssistantMessage></div>}
          </div> : <div className={css.empty}><IconQuestionOutline14 size={28}/><p>{tx('agent:btw.context')}</p></div>}
        </ChatViewport>
      </div>
      <div className={css.composer} data-btw-composer>
        {question && <div className={css.status}><p role="status" aria-live="polite">{tx(states[question.phase])}</p><Button size="sm" type="button" disabled={!available || question.settled} onClick={() => run(() => client.auxiliary.reconcile(sessionId))}>{tx('agent:btw.check')}</Button></div>}
        {question?.diagnostic && question.diagnostic !== 'cancelled' && <p className={css.error} role="alert">{failures[question.diagnostic] ? tx(failures[question.diagnostic]) : question.diagnostic}</p>}
        {error && <p className={css.error} role="alert">{error}</p>}
        <div className={composerCss.card}>
          <div className={composerCss.editor}><textarea className={composerCss.input} aria-label={tx('agent:btw.question')} aria-describedby={hintId} placeholder={tx('agent:btw.question')} value={active ? '' : draft} disabled={!!active || !available} maxLength={16_384} rows={1} onChange={event => { try { client.auxiliary.setDraft(sessionId, event.target.value); setError(undefined); } catch (cause) { setError(String(cause).slice(0, 512)); } }} onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) { event.preventDefault(); submit(); } }}/></div>
          <div className={composerCss.row}>
            <div className={composerCss.trailing}>
              {cancellable ? <button type="button" className={composerCss.primary} aria-label={tx('agent:btw.cancel')} title={tx('agent:btw.cancel')} disabled={!available || question?.cancelling} onClick={() => run(() => client.auxiliary.cancel(sessionId))}><IconStopFill16/></button>
                : <button type="submit" className={composerCss.primary} aria-label={tx('agent:btw.submit')} title={tx('agent:btw.submit')} disabled={!available || !!active || !draft || new TextEncoder().encode(draft).length > 16_384}><IconSendOutline14/></button>}
            </div>
          </div>
        </div>
        <p id={hintId} className={css.hint}>{tx('agent:btw.hint')}</p>
      </div>
      </form>;
}
