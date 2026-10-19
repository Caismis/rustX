import type { CanonicalContentLocator, ContentRef, ContentSource } from './v51.ts';

/** Closed native identities are independent of JSON object property order. */
export function canonicalLocatorKey(locator: CanonicalContentLocator): string {
  return JSON.stringify(locator.type === 'by_message_id'
    ? ['by_message_id', locator.message_id]
    : ['by_ledger_position', locator.position]);
}

function sourceIdentity(source: ContentSource): readonly unknown[] {
  switch (source.type) {
    case 'canonical_message_block':
      return [source.type, source.component, canonicalLocatorKey(source.locator), source.block_index];
    case 'durable_transcript_audit':
    case 'live_publication_prefix':
      return [source.type, source.component, source.stream_id, source.message_id,
        source.attempt_id, source.turn_id, source.publication_request_id, source.block_index];
  }
}

/** Logical content identity; each advancement retains its source and representation. */
export function contentSourceKey(reference: ContentRef): string {
  return JSON.stringify([reference.conversation_id, sourceIdentity(reference.source),
    reference.representation, reference.version]);
}

/** Exact read evidence additionally binds the retained revision and byte frontier. */
export function contentRefKey(reference: ContentRef): string {
  return JSON.stringify([contentSourceKey(reference), reference.content_revision, reference.readable_end]);
}

export function sameContentRef(left: ContentRef, right: ContentRef): boolean {
  return contentRefKey(left) === contentRefKey(right);
}
