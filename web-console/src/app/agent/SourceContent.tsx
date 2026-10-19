import { useEffect, useRef, useState, type ReactNode } from 'react';
import type { AttachmentTarget, ContentBinding, ContentChunk, ContentRef } from '../../../../protocol/app-server/v50';
import { ContentCache } from '../../../../protocol/app-server/content-cache';
import { contentRefKey } from '../../../../protocol/app-server/content-identity';
import type { AppServerClient } from '../../client/app-server';
import { useTranslation } from '../../locale/react';

export async function readContentText(cache: ContentCache, reference: ContentRef, current: () => boolean) {
  const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }), parts: string[] = [];
  let offset: number | undefined = 0;
  do {
    if (!current()) throw Error('Content view was replaced.');
    const chunk = await cache.read(reference, offset, current);
    parts.push(reference.representation === 'binary_bytes' ? Array.from(chunk.bytes, byte => String.fromCharCode(byte)).join('') : decoder.decode(chunk.bytes, { stream: chunk.next !== undefined }));
    offset = chunk.next;
  } while (offset !== undefined);
  return reference.representation === 'binary_bytes' ? btoa(parts.join('')) : parts.join('');
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
  const [state, setState] = useState<{ identity: string; read: typeof read; text?: string[]; error?: string }>();
  const loaded = state?.identity === identity && state.read === read ? state : undefined;
  const active = owned();
  useEffect(() => {
    if (!active || !selected.length || !read && (!client || !target)) return;
    let mounted = true;
    const current = () => mounted && latest.current === identity && latestOwned.current();
    const cache = new ContentCache((ref, offset, _max, current) => read ? read(ref, offset, current) : client!.readContent(target!, agentId, ref, offset, current));
    void (async () => {
      const text: string[] = [];
      for (const binding of selected) {
        text.push(await readContentText(cache, binding.content_ref, current));
      }
      if (current()) setState({ identity, read, text });
    })().catch(error => { if (current()) setState({ identity, read, error: String(error).slice(0, 512) }); });
    return () => { mounted = false; cache.clear(); };
  }, [identity, client, read, active]);
  if (!selected.length) return null;
  if (loaded?.error) return <p role="alert">{loaded.error}</p>;
  return loaded?.text ? <>{loaded.text.map((text, index) => <div key={index}>{children ? children(text) : <pre>{text}</pre>}</div>)}</>
    : fallback ?? <p role="status">{tx('trajectory:details.loading')}</p>;
}
