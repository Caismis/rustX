import { useEffect, useState } from 'react';
import type { AttachmentTarget, MethodResult, SessionNode, SessionSummary } from '../../../../protocol/app-server/v39';
import { sameTarget, type AppServerClient } from '../../client/app-server';
import { sessionDisplayTitle } from '../../bindings/session-title';
import { useTranslation } from '../../locale/react';
import { IconBranchOutline16 } from '../../presentation/primitives/icons';
import css from './ForkPoint.module.css';

export type ForkOrigin = Extract<SessionNode['origin'], { type: 'fork' }>;
export interface ForkPointInfo { origin: ForkOrigin; through: string; source?: SessionSummary; createdAt?: string }

/** Read the attached node, independently of the Session's mutable default.
 * Message remapping and seed placement remain native transcript facts. */
export function useForkPoint(client: AppServerClient, target: AttachmentTarget | undefined, through: string | null | undefined) {
  const [result, setResult] = useState<{ target: AttachmentTarget; point?: ForkPointInfo; error?: string }>();
  const [revision, retry] = useState(0);
  useEffect(() => {
    if (!target || through == null) return;
    let disposed = false;
    const generation = client.getSnapshot().generation;
    const current = () => !disposed && client.getSnapshot().generation === generation && sameTarget(client.getSnapshot().views[target.session_id]?.target, target);
    void (async () => {
      let offset: number | null | undefined = 0;
      while (offset != null && current()) {
        const tree: Extract<MethodResult, { type: 'tree' }> = await client.request({ method: 'session/tree', params: { session_id: target.session_id, offset, limit: 32 } }, 'tree');
        if (!current()) return;
        const node = tree.nodes.find(node => node.conversation_id === target.conversation_id);
        if (!node) { offset = tree.next_offset; continue; }
        client.rememberNode(target, node.id);
        if (node.origin.type !== 'fork') { setResult({ target }); return; }
        const point: ForkPointInfo = { origin: node.origin, through };
        // A cross-Session fork creates the destination Session. A same-Session
        // branch has no native creation timestamp, so never borrow Session age.
        // Deleted metadata does not erase the immutable origin record.
        const [source, destination] = await Promise.allSettled([
          client.request({ method: 'session/summary', params: { session_id: node.origin.source_session } }, 'session_summary'),
          node.origin.source_session !== target.session_id ? client.request({ method: 'session/read', params: { session_id: target.session_id } }, 'session') : Promise.resolve(undefined),
        ]);
        if (source.status === 'fulfilled') point.source = source.value.summary;
        if (destination.status === 'fulfilled') point.createdAt = destination.value?.session.created_at;
        if (current()) setResult({ target, point });
        return;
      }
      if (current()) throw new Error('Attached Conversation is absent from the native Session tree.');
    })().catch(error => { if (current()) setResult({ target, error: String(error) }); });
    return () => { disposed = true; };
  }, [client, target, through, revision]);
  const observed = target && result && sameTarget(target, result.target) ? result : undefined;
  return { point: observed?.point, error: observed?.error, retry: () => retry(value => value + 1) };
}

export function ForkPoint({ point, disabled, onOpen }: { point: ForkPointInfo; disabled?: boolean; onOpen?: (origin: ForkOrigin) => void }) {
  const tx = useTranslation();
  const title = point.source ? sessionDisplayTitle(tx, point.source) : point.origin.source_session;
  const date = point.createdAt ? new Date(point.createdAt) : undefined;
  const time = date && !Number.isNaN(date.getTime()) ? date.toLocaleString(tx.language) : undefined;
  return <div className={css.row} data-chat-anchor-key="fork-point" role="group" aria-label={tx('agent:fork-point.label')}>
    <span className={css.line}/><IconBranchOutline16/>
    <span className={css.label} title={[title, tx(point.origin.side === 'before' ? 'agent:fork-point.before' : 'agent:fork-point.after'), time].filter(Boolean).join('\n')}>
      {tx('agent:fork-point.from', { name: title })}
    </span>
    {onOpen && <button type="button" className={css.open} disabled={disabled} onClick={() => onOpen(point.origin)}>{tx('agent:fork-point.open-source')}</button>}
    <span className={css.line}/>
  </div>;
}
