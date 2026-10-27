import { Fragment, useEffect, useRef, useState, type ReactNode } from 'react';
import type { AttachmentTarget, ContentBinding, ContentChunk, ContentRef } from '../../../../protocol/app-server/v61';
import { ContentCache } from '../../../../protocol/app-server/content-cache';
import { contentRefKey, contentSourceKey } from '../../../../protocol/app-server/content-identity';
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

/** Only a monotonic frontier of the same released live text can retain bytes. */
function advancingPublication(previous: readonly ContentRef[], next: readonly ContentRef[]): boolean {
  return previous.length === next.length && previous.length > 0 && previous.every((before, index) => {
    const after = next[index]!;
    return before.source.type === 'live_publication_prefix' && after.source.type === 'live_publication_prefix'
      && before.source.component === 'payload' && before.representation === 'utf8_text'
      && contentSourceKey(before) === contentSourceKey(after)
      && after.content_revision >= before.content_revision && after.readable_end >= before.readable_end;
  });
}

/** Transport chunks are assembled inside the selected content's normal view. */
export function SourceContent({ client, target, agentId, bindings, owned, read, explicit = false, children, fallback, presentationOwner, inlineText }: {
  client?: AppServerClient; target?: AttachmentTarget; agentId?: string; bindings: ContentBinding[];
  owned: () => boolean; read?: (ref: ContentRef, offset: number, current: () => boolean) => Promise<ContentChunk>;
  explicit?: boolean; children?: (text: string) => ReactNode; fallback?: ReactNode;
  /** A logical presentation scope retains released text, never read capabilities. */
  presentationOwner?: symbol; inlineText?: string;
}) {
  const tx = useTranslation(), latestOwned = useRef(owned); latestOwned.current = owned;
  const selected = bindings.filter(binding => explicit || binding.deferred);
  const scope = JSON.stringify([target, agentId]), references = selected.map(binding => binding.content_ref);
  const identity = JSON.stringify([scope, references.map(contentRefKey)]);
  const latest = useRef({ identity, client, read, presentationOwner }); latest.current = { identity, client, read, presentationOwner };
  const [state, setState] = useState<{ identity: string; scope: string; client: typeof client; read: typeof read; presentationOwner?: symbol; references: ContentRef[]; text: string[] }>();
  const [failure, setFailure] = useState<{ identity: string; client: typeof client; read: typeof read; presentationOwner?: symbol; message: string }>();
  const sameOwner = state?.scope === scope && state.client === client && state.read === read && state.presentationOwner === presentationOwner;
  const sealed = sameOwner && sealedPublication(state.references, references);
  const active = owned();
  const loaded = active && sameOwner && (state.identity === identity || sealed || advancingPublication(state.references, references)) ? state : undefined;
  const error = active && failure?.identity === identity && failure.client === client && failure.read === read && failure.presentationOwner === presentationOwner ? failure.message : undefined;
  const released = useRef<{ owner: symbol; references: ContentRef[]; text: string[] }>(undefined);
  const displayReferences = selected.length ? references : bindings.map(binding => binding.content_ref);
  const previous = released.current;
  const exact = previous && previous.references.length === displayReferences.length && previous.references.every((ref, index) => contentRefKey(ref) === contentRefKey(displayReferences[index]));
  if (!presentationOwner || previous && (previous.owner !== presentationOwner || !exact && !sealedPublication(previous.references, displayReferences) && !advancingPublication(previous.references, displayReferences))) released.current = undefined;
  if (loaded && presentationOwner) released.current = { owner: presentationOwner, references: loaded.references, text: loaded.text };
  if (!selected.length && presentationOwner && inlineText !== undefined && displayReferences.length === 1 && displayReferences[0].representation === 'utf8_text' && new TextEncoder().encode(inlineText).length === displayReferences[0].readable_end) released.current = { owner: presentationOwner, references: displayReferences, text: [inlineText] };
  useEffect(() => {
    if (!active || !selected.length) { setState(undefined); setFailure(undefined); return; }
    if (!read && (!client || !target) || sealed) return;
    setFailure(undefined);
    let mounted = true;
    const current = () => mounted && latest.current.identity === identity && latest.current.client === client && latest.current.read === read && latest.current.presentationOwner === presentationOwner && latestOwned.current();
    const cache = new ContentCache((ref, offset, _max, current) => read ? read(ref, offset, current) : client!.reads.readContent(target!, agentId, ref, offset, current));
    void (async () => {
      const text: string[] = [];
      for (const binding of selected) {
        text.push(await readContentText(cache, binding.content_ref, current));
      }
      if (current()) setState({ identity, scope, client, read, presentationOwner, references, text });
    })().catch(error => { if (current()) setFailure({ identity, client, read, presentationOwner, message: String(error).slice(0, 512) }); });
    return () => { mounted = false; cache.clear(); };
  }, [identity, client, read, active, presentationOwner]);
  if (!selected.length && inlineText === undefined) return null;
  const text = selected.length ? (loaded ?? released.current)?.text : [inlineText!];
  return <>{error && <p role="alert">{error}</p>}
    {text ? text.map((text, index) => <Fragment key={index}>{children ? children(text) : <pre>{text}</pre>}</Fragment>)
      : !error && <Fragment key={0}>{fallback ?? <p role="status">{tx('trajectory:details.loading')}</p>}</Fragment>}
  </>;
}
