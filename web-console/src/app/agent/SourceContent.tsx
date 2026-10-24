import { Fragment, useEffect, useRef, useState, type ReactNode } from 'react';
import type { AttachmentTarget, ContentBinding, ContentChunk, ContentRef } from '../../../../protocol/app-server/v57';
import { ContentCache } from '../../../../protocol/app-server/content-cache';
import { contentRefKey } from '../../../../protocol/app-server/content-identity';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';

export { readContentText } from '../../client/content-text';
import { readContentText } from '../../client/content-text';

/** An immutable audit seals exactly the released bytes of its named stream. */
function sealedPublication(previous: readonly ContentRef[], next: readonly ContentRef[]): boolean {
  return previous.length === next.length && previous.length > 0 && previous.every((before,index) => {
    const after = next[index]!, from = before.source, to = after.source;
    return from.type === 'live_publication_prefix' && to.type === 'durable_transcript_audit'
      && before.conversation_id === after.conversation_id && before.representation === after.representation
      && before.version === after.version && before.readable_end === after.readable_end
      && from.component === 'payload' && to.component === 'payload' && from.block_index === to.block_index
      && from.stream_id === to.stream_id && from.message_id === to.message_id
      && from.attempt_id === to.attempt_id && from.turn_id === to.turn_id
      && from.publication_request_id === to.publication_request_id;
  });
}

/** Transport chunks are assembled inside the selected content's normal view. */
export function SourceContent({ client, target, agentId, bindings, owned, read, explicit = false, children, fallback }: {
  client?: AppServerClient; target?: AttachmentTarget; agentId?: string; bindings: ContentBinding[];
  owned: () => boolean; read?: (ref: ContentRef, offset: number, current: () => boolean) => Promise<ContentChunk>;
  explicit?: boolean; children?: (text: string) => ReactNode; fallback?: ReactNode;
}) {
  const tx = useTranslation(), latestOwned = useRef(owned); latestOwned.current = owned;
  const selected = bindings.filter(binding => explicit || binding.deferred);
  const identity = JSON.stringify([target, agentId, selected.map(binding => contentRefKey(binding.content_ref))]);
  const latest = useRef(identity); latest.current = identity;
  const [state, setState] = useState<{ identity: string; read: typeof read; references: ContentRef[]; text?: string[]; error?: string }>();
  const sealed = !!state?.text && sealedPublication(state.references, selected.map(binding => binding.content_ref));
  const loaded = state && state.read === read && (state.identity === identity || sealed) ? state : undefined;
  const active = owned();
  useEffect(() => {
    if (!active || !selected.length || !read && (!client || !target)) return;
    if (sealed && state?.read === read) return;
    let mounted = true;
    const current = () => mounted && latest.current === identity && latestOwned.current();
    const cache = new ContentCache((ref, offset, _max, current) => read ? read(ref, offset, current) : client!.reads.readContent(target!, agentId, ref, offset, current));
    void (async () => {
      const text: string[] = [];
      for (const binding of selected) {
        text.push(await readContentText(cache, binding.content_ref, current));
      }
      if (current()) setState({ identity, read, references: selected.map(binding => binding.content_ref), text });
    })().catch(error => { if (current()) setState({ identity, read, references: selected.map(binding => binding.content_ref), error: String(error).slice(0, 512) }); });
    return () => { mounted = false; cache.clear(); };
  }, [identity, client, read, active]);
  if (!selected.length) return null;
  if (loaded?.error) return <p role="alert">{loaded.error}</p>;
  return loaded?.text ? <>{loaded.text.map((text, index) => <Fragment key={index}>{children ? children(text) : <pre>{text}</pre>}</Fragment>)}</>
    : fallback ?? <p role="status">{tx('trajectory:details.loading')}</p>;
}
