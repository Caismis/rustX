import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from MessageIconActions; see PROVENANCE.md. */
import { useEffect, useState } from 'react';
import type { CompletedResponseView } from '../../../../protocol/app-server/v36';
import { writeClipboard } from '../../presentation/primitives/clipboard';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { IconCopyOutline16, IconCheckOutline16, IconBranchOutline16, IconRefreshOutline16 } from '../../presentation/primitives/icons';
import type { ResponseAction } from '../commands/native';
import { turnAnchor } from '../../client/transcript';
import { TurnUsage } from './UsageStats';
import css from './TurnTail.module.css';

export function MessageTime({ time }: { time?: string | null }) {
  const tx = useTranslation();
  return time ? <time className={css.time} dateTime={time} title={new Date(time).toLocaleString(tx.language)}>{new Date(time).toLocaleTimeString(tx.language, { hour: '2-digit', minute: '2-digit' })}</time> : null;
}
export function CopyMessage({ text }: { text: string }) {
  const tx = useTranslation();
  const [status, setStatus] = useState<'idle' | 'copied' | 'failed'>('idle');
  const label = status === 'idle' ? tx('agent:copy.copy') : status === 'copied' ? tx('agent:copy.copied') : tx('agent:copy.copy-failed');
  useEffect(() => {
    if (status === 'idle') return;
    const timer = setTimeout(() => setStatus('idle'), 3000);
    return () => clearTimeout(timer);
  }, [status]);
  return <Tooltip label={label} side="bottom"><button type="button" className={css.action} aria-label={label} onClick={() => {
    void writeClipboard(text).then(ok => setStatus(ok ? 'copied' : 'failed'));
  }}>{status === 'copied' ? <IconCheckOutline16/> : <IconCopyOutline16/>}</button></Tooltip>;
}
export function TurnTail({ text, response, onHistorical, disabled, lineageSwitchSafe, latest = false }: { text: string; response: CompletedResponseView; onHistorical?: (action: ResponseAction, response: CompletedResponseView) => void; disabled?: boolean; lineageSwitchSafe: boolean; latest?: boolean }) {
  const tx = useTranslation();
  return <div className={css.actions} aria-label={tx('agent:turn-tail.completed-turn')} data-chat-turn-owner={turnAnchor(response.origin)} data-response-reveal={latest ? 'always' : 'hover'} data-turn-tail={JSON.stringify([response.origin.conversation_id, response.origin.attempt_id])}>
    <CopyMessage text={text}/>
    {onHistorical && <>
      {/* rustX's Retry replays the original input on a sibling branch; it sits
          where Harness seats extra actions, between copy and branch. */}
      {response.retry_message_id && <Tooltip label={tx('agent:turn-tail.retry')} side="bottom"><button className={css.action} type="button" aria-label={tx('agent:turn-tail.retry')} disabled={disabled || !lineageSwitchSafe} onClick={() => onHistorical('retry', response)}><IconRefreshOutline16/></button></Tooltip>}
      <Tooltip label={tx('agent:turn-tail.branch')} side="bottom"><button className={css.action} type="button" aria-label={tx('agent:turn-tail.branch')} disabled={disabled} onClick={() => onHistorical('fork', response)}><IconBranchOutline16/></button></Tooltip>
    </>}
    {response.usage && <TurnUsage usage={response.usage} models={response.models ?? []}/>}
    <MessageTime time={response.completed_at}/>
  </div>;
}
