/* Copyright (c) 2026 DeepSeek. MIT. Adapted from MessageIconActions and TurnUsagePanel; see PROVENANCE.md. */
import { useState } from 'react';
import type { CompletedResponseView, ModelUsage } from '../../../../protocol/app-server/v8';
import { writeClipboard } from '../../presentation/primitives/clipboard';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { Modal } from '../../presentation/primitives/Modal';
import { Button } from '../../presentation/primitives/Button';
import { IconCopyOutline16, IconCheckOutline16, IconBranchOutline16, IconDatabaseOutline16 } from '../../presentation/primitives/icons';
import type { HistoryAction } from '../commands/native';
import css from './ResponseTail.module.css';

export function MessageTime({ time }: { time?: string | null }) {
  return time ? <time className={css.time} dateTime={time} title={new Date(time).toLocaleString()}>{new Date(time).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time> : null;
}
export function CopyMessage({ text }: { text: string }) {
  const [status, setStatus] = useState('Copy');
  return <Tooltip label={status} side="bottom"><button type="button" className={css.action} aria-label={status} onClick={() => {
    void writeClipboard(text).then(ok => setStatus(ok ? 'Copied' : 'Copy failed'));
  }}>{status === 'Copied' ? <IconCheckOutline16/> : <IconCopyOutline16/>}</button></Tooltip>;
}
export const compactTokens = (value: number) => new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 }).format(value);
export function Usage({ usage, label = 'Usage', showCache = false }: { usage: ModelUsage; label?: string; showCache?: boolean }) {
  const [open, setOpen] = useState(false);
  const cached = usage.details?.cached_input_tokens;
  return <><button className={css.stat} type="button" aria-haspopup="dialog" aria-expanded={open} aria-label={`${label} ${usage.total_tokens} tokens`} onClick={() => setOpen(true)}><IconDatabaseOutline16/><span>{compactTokens(usage.total_tokens)} tok{showCache && cached != null && usage.input_tokens > 0 && cached <= usage.input_tokens ? ` · ${Math.round(100 * cached / usage.input_tokens)}% cache` : ''}</span></button>
    <Modal open={open} title={label} closeLabel="Close usage" onClose={() => setOpen(false)}><dl className={css.metrics}>
      <dt>Total</dt><dd>{usage.total_tokens.toLocaleString()}</dd>
      <dt>Input</dt><dd>{usage.input_tokens.toLocaleString()}</dd>
      {cached != null && <><dt>Uncached input</dt><dd>{(usage.input_tokens - cached).toLocaleString()}</dd><dt>Cache read</dt><dd>{cached.toLocaleString()}</dd>{usage.input_tokens > 0 && <><dt>Cache hit</dt><dd>{(100 * cached / usage.input_tokens).toFixed(1)}%</dd></>}</>}
      <dt>Output</dt><dd>{usage.output_tokens.toLocaleString()}</dd>
      {usage.details?.reasoning_tokens != null && <><dt>Reasoning</dt><dd>{usage.details.reasoning_tokens.toLocaleString()}</dd></>}
    </dl></Modal></>;
}
export function ResponseTail({ text, response, onHistorical, disabled, lineageSwitchSafe }: { text: string; response: CompletedResponseView; onHistorical?: (action: HistoryAction, response: CompletedResponseView) => void; disabled?: boolean; lineageSwitchSafe: boolean }) {
  const [open, setOpen] = useState(false);
  return <div className={css.actions} aria-label="Completed response">
    <CopyMessage text={text}/>
    {onHistorical && <><Tooltip label="Lineage" side="bottom"><button className={css.action} type="button" aria-label="Lineage" aria-haspopup="dialog" aria-expanded={open} disabled={disabled} onClick={() => setOpen(true)}><IconBranchOutline16/></button></Tooltip>
      <Modal open={open} title="Continue this conversation" closeLabel="Close lineage" onClose={() => setOpen(false)}><div className={css.menu}>
        <Button disabled={disabled || !lineageSwitchSafe} onClick={() => { setOpen(false); onHistorical('branch', response); }}>Branch in this Session</Button>
        <Button disabled={disabled} onClick={() => { setOpen(false); onHistorical('fork', response); }}>Fork to new Session</Button>
        {response.retry_message_id && <Button disabled={disabled || !lineageSwitchSafe} onClick={() => { setOpen(false); onHistorical('retry', response); }}>Retry / Regenerate</Button>}
      </div></Modal></>}
    {response.usage && <Usage usage={response.usage}/>}
    <MessageTime time={response.completed_at}/>
  </div>;
}
