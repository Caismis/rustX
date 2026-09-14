/**
 * The two boundary checks the projection performs before folding a fact.
 *
 * These are client-owned presentation rules, not a second copy of the wire
 * contract: they decide which runtime facts belong in a transcript and refuse
 * to fold a fact that contradicts itself, so a malformed pairing surfaces at
 * the boundary rather than as an unexplained gap on screen.
 */

import type { RuntimeClientTranscriptCursor } from "../protocol/app-server.ts";

/** Whether a User message is a hidden Context fact rather than transcript content. */
export function isHiddenContextMessage(message: {
  role?: unknown;
  kind?: unknown;
}): boolean {
  const isUserMessage = message.role === undefined || message.role === "user";
  return (
    isUserMessage &&
    typeof message.kind === "object" &&
    message.kind !== null &&
    "context" in message.kind
  );
}

/**
 * Whether a value is a canonical transcript cursor.
 *
 * The cursor is an exact `u64` domain and therefore canonical unsigned decimal
 * **text** on the wire: `"9007199254740993"` must survive, which a JSON number
 * could not. Canonicity is checked by round-tripping through `BigInt`, so
 * leading zeros, signs and whitespace are all rejected.
 */
function isWireTranscriptCursor(
  value: unknown,
): value is RuntimeClientTranscriptCursor {
  if (typeof value !== "string" || !/^(0|[1-9][0-9]*)$/.test(value)) {
    return false;
  }
  try {
    return BigInt(value) >= 0n;
  } catch {
    return false;
  }
}

/** Checks the one visible/hidden transcript-cursor contract at the wire boundary. */
export function hasTranscriptCursorContract(
  message: unknown,
  transcriptCursor: unknown,
): boolean {
  if (typeof message !== "object" || message === null) {
    return false;
  }
  if (isHiddenContextMessage(message as { role?: unknown; kind?: unknown })) {
    return transcriptCursor === undefined || transcriptCursor === null;
  }
  return isWireTranscriptCursor(transcriptCursor);
}

/**
 * Validates a transcript-visible message before presentation reduction.
 *
 * Hidden Context messages may omit the cursor and never enter the ordinary
 * transcript. Every other message must carry the durable cursor allocated by
 * `transcript_order`; contradictory hidden-with-cursor facts fail closed.
 */
export function validateTranscriptCursorContract(
  message: { role?: unknown; kind?: unknown },
  transcriptCursor: RuntimeClientTranscriptCursor | undefined | null,
): RuntimeClientTranscriptCursor | undefined {
  if (isHiddenContextMessage(message)) {
    if (transcriptCursor !== undefined && transcriptCursor !== null) {
      throw new Error(
        "hidden Context message must not carry a durable transcript cursor",
      );
    }
    return undefined;
  }
  if (!isWireTranscriptCursor(transcriptCursor)) {
    throw new Error(
      "visible transcript message is missing a valid durable transcript cursor",
    );
  }
  return transcriptCursor;
}
