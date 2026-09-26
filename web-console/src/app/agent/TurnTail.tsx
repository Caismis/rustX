import type { Translate } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from MessageIconActions and TurnUsagePanel; see PROVENANCE.md. */
import { useEffect, useState } from 'react';
import type { CompletedResponseView, CompletedResponseTiming, ModelUsage } from '../../../../protocol/app-server/v23';
import { writeClipboard } from '../../presentation/primitives/clipboard';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { Modal } from '../../presentation/primitives/Modal';
import { IconClockOutline16, IconCopyOutline16, IconCheckOutline16, IconBranchOutline16, IconDatabaseOutline16 } from '../../presentation/primitives/icons';
import type { HistoryAction } from '../commands/native';
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
export const compactTokens = (tx: Translate, value: number) => new Intl.NumberFormat(tx.language, { notation: 'compact', maximumFractionDigits: 1 }).format(value);
export function Usage({ usage, label: suppliedLabel, showCache = false }: { usage: ModelUsage; label?: string; showCache?: boolean }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const label = suppliedLabel ?? tx('agent:copy.usage');
  const cached = usage.details?.cached_input_tokens;
  return <><button className={css.stat} type="button" aria-haspopup="dialog" aria-expanded={open} aria-label={tx('agent:turn-tail.value-value-tokens', { p0: label, p1: usage.total_tokens })} onClick={() => setOpen(true)}><IconDatabaseOutline16/><span>{compactTokens(tx, usage.total_tokens)} {tx('agent:turn-tail.tok')}{showCache && cached != null && usage.input_tokens > 0 && cached <= usage.input_tokens ? tx('agent:turn-tail.value-cache', { p0: Math.round(100 * cached / usage.input_tokens) }) : ''}</span></button>
    <Modal open={open} title={label} closeLabel={tx('agent:turn-tail.close-usage')} onClose={() => setOpen(false)}><dl className={css.metrics}>
      <dt>{tx('agent:turn-tail.total')}</dt><dd>{usage.total_tokens.toLocaleString(tx.language)}</dd>
      <dt>{tx('agent:turn-tail.input')}</dt><dd>{usage.input_tokens.toLocaleString(tx.language)}</dd>
      {cached != null && <><dt>{tx('agent:turn-tail.uncached-input')}</dt><dd>{(usage.input_tokens - cached).toLocaleString(tx.language)}</dd><dt>{tx('agent:turn-tail.cache-read')}</dt><dd>{cached.toLocaleString(tx.language)}</dd>{usage.input_tokens > 0 && <><dt>{tx('agent:turn-tail.cache-hit')}</dt><dd>{(100 * cached / usage.input_tokens).toFixed(1)}%</dd></>}</>}
      <dt>{tx('agent:turn-tail.output')}</dt><dd>{usage.output_tokens.toLocaleString(tx.language)}</dd>
      {usage.details?.reasoning_tokens != null && <><dt>{tx('agent:turn-tail.reasoning')}</dt><dd>{usage.details.reasoning_tokens.toLocaleString(tx.language)}</dd></>}
    </dl></Modal></>;
}
const duration = (tx: Translate, ms: number) => tx('agent:duration.seconds-decimal', { n: new Intl.NumberFormat(tx.language, { maximumFractionDigits: 2 }).format(ms / 1000) });
function Timing({ timing }: { timing: CompletedResponseTiming }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  if (timing.total_duration_ms == null) return null;
  const label = tx('agent:copy.ran-for-value', { p0: duration(tx, timing.total_duration_ms) });
  return <><button className={css.stat} type="button" aria-label={label} aria-haspopup="dialog" aria-expanded={open} onClick={() => setOpen(true)}><IconClockOutline16/><span>{label}</span></button>
    <Modal open={open} title={tx('agent:turn-tail.response-timing')} closeLabel={tx('agent:turn-tail.close-timing')} onClose={() => setOpen(false)}><dl className={css.metrics}>
      <dt>{tx('agent:turn-tail.total-runtime')}</dt><dd>{duration(tx, timing.total_duration_ms)}</dd>
      {timing.ttft_ms != null && <><dt>{tx('agent:turn-tail.first-request-ttft-from-dispatch')}</dt><dd>{duration(tx, timing.ttft_ms)}</dd></>}
      {timing.generation_ms != null && <><dt>{tx('agent:conversation-stats.model-generation-work')}</dt><dd>{duration(tx, timing.generation_ms)}</dd></>}
      {timing.output_tokens_per_second != null && <><dt>{tx('agent:turn-tail.output-speed')}</dt><dd>{timing.output_tokens_per_second.toLocaleString(tx.language, { maximumFractionDigits: 1 })} {tx('agent:conversation-stats.tok-s')}</dd></>}
    </dl></Modal></>;
}
export function TurnTail({ text, response, onHistorical, disabled, lineageSwitchSafe, latest = false }: { text: string; response: CompletedResponseView; onHistorical?: (action: HistoryAction, response: CompletedResponseView) => void; disabled?: boolean; lineageSwitchSafe: boolean; latest?: boolean }) {
  const tx = useTranslation();
  return <div className={css.actions} aria-label={tx('agent:turn-tail.completed-turn')} data-response-reveal={latest ? 'always' : 'hover'} data-turn-tail={JSON.stringify([response.origin.conversation_id, response.origin.attempt_id])}>
    <CopyMessage text={text}/>
    {onHistorical && <>
      <Tooltip label={tx('agent:turn-tail.fork-to-new-session')} side="bottom"><button className={css.action} type="button" aria-label={tx('agent:turn-tail.fork-to-new-session')} disabled={disabled} onClick={() => onHistorical('fork', response)}><IconBranchOutline16/></button></Tooltip>
      <Tooltip label={tx('agent:turn-tail.branch-in-this-session')} side="bottom"><button className={css.action} type="button" aria-label={tx('agent:turn-tail.branch-in-this-session')} disabled={disabled || !lineageSwitchSafe} onClick={() => onHistorical('branch', response)}><IconBranchOutline16/></button></Tooltip>
      {response.retry_message_id && <Tooltip label={tx('agent:turn-tail.retry-regenerate')} side="bottom"><button className={css.action} type="button" aria-label={tx('agent:turn-tail.retry-regenerate')} disabled={disabled || !lineageSwitchSafe} onClick={() => onHistorical('retry', response)}>↻</button></Tooltip>}
    </>}
    {response.usage && <Usage usage={response.usage}/>}
    {response.timing && <Timing timing={response.timing}/>}
    <MessageTime time={response.completed_at}/>
  </div>;
}
