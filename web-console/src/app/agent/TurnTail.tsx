import { attemptOwnerKey } from '../../../../protocol/app-server/attempt-identity';
import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. Adapted from MessageIconActions; see PROVENANCE.md. */
import { useEffect, useState, useRef } from 'react';
import type { CompletedResponseView, PublicationAudit } from '../../../../protocol/app-server/v57';
import { writeClipboard } from '../../presentation/primitives/clipboard';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { IconCopyOutline16, IconCheckOutline16, IconBranchOutline16, IconRefreshOutline16 } from '../../presentation/primitives/icons';
import type { ResponseAction } from '../commands/native';
import { turnAnchor } from '../../client/transcript';
import { TurnUsage } from './UsageStats';
import css from './TurnTail.module.css';
import { messageContentBinding, observationSourceIdentity, useContentSource } from './ObservationSource';
import { ContentCache } from '../../../../protocol/app-server/content-cache';
import { readContentText } from './SourceContent';

export function MessageTime({ time }: { time?: string | null }) {
  const tx = useTranslation();
  return time ? <time className={css.time} dateTime={time} title={new Date(time).toLocaleString(tx.language)}>{new Date(time).toLocaleTimeString(tx.language, { hour: '2-digit', minute: '2-digit' })}</time> : null;
}
export function CopyMessage({ text, messageId, publication }: { text: string; messageId?: string | null; publication?: PublicationAudit }) {
  const tx = useTranslation();
  const { source, read } = useContentSource(), latest = useRef(source); latest.current = source;
  const mounted = useRef(true); useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const [status, setStatus] = useState<'idle' | 'copied' | 'failed'>('idle');
  const label = status === 'idle' ? tx('agent:copy.copy') : status === 'copied' ? tx('agent:copy.copied') : tx('agent:copy.copy-failed');
  useEffect(() => {
    if (status === 'idle') return;
    const timer = setTimeout(() => setStatus('idle'), 3000);
    return () => clearTimeout(timer);
  }, [status]);
  return <Tooltip label={label} side="bottom"><button type="button" className={css.action} aria-label={label} onClick={() => {
    const identity = JSON.stringify(observationSourceIdentity(source));
    const current = () => mounted.current && (!source || latest.current?.client === source.client && JSON.stringify(observationSourceIdentity(latest.current)) === identity && source.owned());
    void (async () => {
      let value = text;
      if (publication && source) {
        const cache = new ContentCache((ref, offset, _max, current) => read(ref, offset, current));
        try {
          const parts: string[] = [];
          for (const block of publication.content) if (block.kind === 'text' || block.kind === 'refusal') {
            const binding = messageContentBinding(source, publication.message_id, block.block_index, publication.stream_id);
            parts.push(binding ? await readContentText(cache, binding.content_ref, current) : block.text);
          }
          value = parts.join('');
        } finally { cache.clear(); }
      }
      const message = source?.messages?.find(message => message.id === messageId) ?? [source?.transcript, source?.history].flatMap(page => page?.entries?.flatMap(entry => entry.item.type === 'message' ? [entry.item.message] : []) ?? []).find(message => message.id === messageId);
      if (message && message.role !== 'tool' && source) {
        const cache = new ContentCache((ref, offset, _max, current) => read(ref, offset, current));
        try {
          const parts: string[] = [];
          for (const [index, block] of message.content.entries()) if (block.type === 'text') {
            const binding = messageContentBinding(source, message.id, index);
            parts.push(binding ? await readContentText(cache, binding.content_ref, current) : block.text);
          }
          value = parts.join('');
        } finally { cache.clear(); }
      }
      if (current()) { const ok = await writeClipboard(value); if (current()) setStatus(ok ? 'copied' : 'failed'); }
    })().catch(() => { if (current()) setStatus('failed'); });
  }}>{status === 'copied' ? <IconCheckOutline16/> : <IconCopyOutline16/>}</button></Tooltip>;
}
export function TurnTail({ text, response, onHistorical, disabled, lineageSwitchSafe, latest = false }: { text: string; response: CompletedResponseView; onHistorical?: (action: ResponseAction, response: CompletedResponseView) => void; disabled?: boolean; lineageSwitchSafe: boolean; latest?: boolean }) {
  const tx = useTranslation();
  return <div className={css.actions} aria-label={tx('agent:turn-tail.completed-turn')} data-chat-turn-owner={turnAnchor(response.origin)} data-response-reveal={latest ? 'always' : 'hover'} data-turn-tail={attemptOwnerKey(response.origin)}>
    <CopyMessage text={text} messageId={response.closing_message_id}/>
    {onHistorical && <>
      {/* rustX's Retry replays the original input on a sibling branch; it sits
          where Harness seats extra actions, between copy and branch. */}
      {response.retry_locator && <Tooltip label={tx('agent:turn-tail.retry')} side="bottom"><button className={css.action} type="button" aria-label={tx('agent:turn-tail.retry')} disabled={disabled || !lineageSwitchSafe} onClick={() => onHistorical('retry', response)}><IconRefreshOutline16/></button></Tooltip>}
      <Tooltip label={tx('agent:turn-tail.branch')} side="bottom"><button className={css.action} type="button" aria-label={tx('agent:turn-tail.branch')} disabled={disabled} onClick={() => onHistorical('fork', response)}><IconBranchOutline16/></button></Tooltip>
    </>}
    {response.usage && <TurnUsage usage={response.usage} models={response.models ?? []}/>}
    <MessageTime time={response.completed_at}/>
  </div>;
}
