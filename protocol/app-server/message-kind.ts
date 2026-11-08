import type {InboundKind} from './v76.ts';

/** Mailbox queue intent is still ordinary user content in every client surface. */
export function isPlainMessageKind(kind: InboundKind | null | undefined): kind is 'message' | 'queued_message' | null | undefined {
  return kind == null || kind === 'message' || kind === 'queued_message';
}
