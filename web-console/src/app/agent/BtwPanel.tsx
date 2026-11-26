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
import { composerCss } from '../../presentation/agent/composerCss';
import { Textarea } from '@/components/ui/textarea';
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
  return <form ref={root} className="flex flex-col w-full h-full min-w-0 min-h-0 overflow-hidden" aria-label={tx('agent:btw.title')} tabIndex={-1} onSubmit={event => { event.preventDefault(); submit(); }} onKeyDown={event => {
    if (event.key !== 'Escape' || event.defaultPrevented || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229 || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    event.preventDefault(); event.stopPropagation(); if (!event.repeat) close();
  }}>
      <header className="flex items-center flex-none gap-[8px] px-[16px] py-[12px] [color:var(--dsw-alias-label-tertiary)] text-[12px] leading-[18px]">
        <span className="inline-flex items-center gap-[4px] px-[7px] py-[2px] [border:0.5px_solid_var(--dsw-alias-border-l2)] rounded-[var(--dsw-radius-sm)] [color:var(--dsw-alias-label-secondary)]"><IconQuestionOutline14/>{tx('agent:btw.temporary')}</span>
        <span title={tx('agent:btw.context')}>{tx('agent:btw.snapshot')}</span>
      </header>
      <div className="relative flex flex-1 min-h-0 min-w-0 overflow-hidden [&_.conversation-scroll]:min-h-0 [&_.conversation-scroll]:px-[16px] [&_.conversation-scroll]:pt-[12px] [&_.conversation-scroll]:pb-[24px] [&_.conversation-scroll>div]:flex [&_.conversation-scroll>div]:flex-col [&_.conversation-scroll>div]:min-h-full [&_.chat-return-latest]:right-[16px] [&_.chat-return-latest]:bottom-[12px]" data-btw-transcript>
        <ChatViewport resetKey={question?.correlation ?? sessionId} chromeVisible={visible} latestLabel={tx('agent:agent-transcript.return-to-latest')}>
          {question ? <div className="flex flex-col gap-[24px] min-w-0 wrap-anywhere">
            <div data-chat-anchor-key={`${question.correlation}:question`} data-chat-flow-kind="user"><UserMessage label={tx('agent:message.your-message')}>{question.question}</UserMessage></div>
            {question.text && <div data-chat-anchor-key={`${question.correlation}:answer`}><AssistantMessage label={tx('agent:message.assistant-response')}><div data-btw-complete={question.complete}><MarkdownText text={question.text} streaming={!question.settled}/></div></AssistantMessage></div>}
          </div> : <div className="flex flex-col items-center justify-center flex-1 gap-[12px] px-[12px] py-[24px] [color:var(--dsw-alias-label-tertiary)] text-[13px] leading-[20px] text-center [&_p]:max-w-[320px] [&_p]:m-0"><IconQuestionOutline14 size={28}/><p>{tx('agent:btw.context')}</p></div>}
        </ChatViewport>
      </div>
      <div className="[--dsh-composer-text-max-height:min(128px,25vh)] flex flex-col flex-none gap-[8px] px-[12px] pt-[10px] pb-[12px] [border-top:0.5px_solid_var(--dsw-alias-border-l2)]" data-btw-composer>
        {question && <div className="flex items-center justify-between gap-[8px] min-w-0 text-[12px] leading-[18px] [color:var(--dsw-alias-label-tertiary)] [&_p]:min-w-0 [&_p]:m-0 [&_p]:wrap-anywhere [&_button]:flex-none"><p role="status" aria-live="polite">{tx(states[question.phase])}</p><Button size="sm" type="button" disabled={!available || question.settled} onClick={() => run(() => client.auxiliary.reconcile(sessionId))}>{tx('agent:btw.check')}</Button></div>}
        {question?.diagnostic && question.diagnostic !== 'cancelled' && <p className="max-h-[64px] m-0 overflow-auto wrap-anywhere [color:var(--dsw-alias-state-error-primary)] text-[12px] leading-[18px]" role="alert">{failures[question.diagnostic] ? tx(failures[question.diagnostic]) : question.diagnostic}</p>}
        {error && <p className="max-h-[64px] m-0 overflow-auto wrap-anywhere [color:var(--dsw-alias-state-error-primary)] text-[12px] leading-[18px]" role="alert">{error}</p>}
        <div className={composerCss.card}>
          <div className={composerCss.editor}><Textarea unstyled className={composerCss.input} aria-label={tx('agent:btw.question')} aria-describedby={hintId} placeholder={tx('agent:btw.question')} value={active ? '' : draft} disabled={!!active || !available} maxLength={16_384} rows={1} onChange={event => { try { client.auxiliary.setDraft(sessionId, event.target.value); setError(undefined); } catch (cause) { setError(String(cause).slice(0, 512)); } }} onKeyDown={event => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.nativeEvent.isComposing && event.nativeEvent.keyCode !== 229) { event.preventDefault(); submit(); } }}/></div>
          <div className={composerCss.row}>
            <div className={composerCss.trailing}>
              {cancellable ? <button type="button" className={composerCss.primary} aria-label={tx('agent:btw.cancel')} title={tx('agent:btw.cancel')} disabled={!available || question?.cancelling} onClick={() => run(() => client.auxiliary.cancel(sessionId))}><IconStopFill16/></button>
                : <button type="submit" className={composerCss.primary} aria-label={tx('agent:btw.submit')} title={tx('agent:btw.submit')} disabled={!available || !!active || !draft || new TextEncoder().encode(draft).length > 16_384}><IconSendOutline14/></button>}
            </div>
          </div>
        </div>
        <p id={hintId} className="m-0 [color:var(--dsw-alias-label-tertiary)] text-[11px] leading-[16px]">{tx('agent:btw.hint')}</p>
      </div>
      </form>;
}
