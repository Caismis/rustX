import type { CanonicalContentLocator, CheckpointReadCut, ContentRef, ContentSource, ObservationReadCut } from './v57.ts';

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

/** Exact durable frontier, independent of transport object property order. */
export function observationReadCutKey(cut: ObservationReadCut): string {
  const read = cut.read;
  return JSON.stringify([read.conversation_id, read.journal, read.transcript, read.mutation_revision,
    read.ledger_through, read.surface_revision, read.transcript_total, read.pending_through, read.pending_total,
    cut.ledger_through, cut.transcript_total, cut.surface_revision, cut.inherited_through]);
}

/** Closed read evidence normalizes optional native selectors and map ordering. */
export function checkpointReadCutKey(cut: CheckpointReadCut): string {
  const foreground = cut.canonical_foreground, publication = cut.live_publication;
  return JSON.stringify([cut.conversation_id, cut.activation_id ?? null, cut.revision, cut.through,
    cut.source_cut ? observationReadCutKey(cut.source_cut) : null,
    foreground ? [canonicalLocatorKey(foreground.locator), foreground.total] : null,
    publication ? [sourceIdentity(publication.source), publication.revision, publication.block_total, publication.tool_total] : null,
    Object.keys(cut.domain_revisions).sort().map(key => [key, cut.domain_revisions[key]]),
    Object.keys(cut.collection_cuts).sort().map(key => {
      const collection = cut.collection_cuts[key]!;
      return [key, collection.generation, collection.total];
    })]);
}
