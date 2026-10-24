/* Copyright (c) 2026 DeepSeek. MIT. History window policy adapted from session-controller; see PROVENANCE.md. */
import type { RuntimeClientTranscriptEntry, TraceRecord } from '../../../protocol/app-server/v57';
import { attemptOwnerKey } from '../../../protocol/app-server/attempt-identity';
import { traceAttemptKey } from '../../../protocol/app-server/trace-identity';

// DSH loads at least 50 messages and two Turns, with a 500-message ceiling.
// rustX transports bounded record pages, so assemble that reading window from
// native pages rather than exposing a manual load action for each wire chunk.
export const HISTORY_MIN_MESSAGES = 50;
export const HISTORY_MIN_TURNS = 2;
export const HISTORY_MAX_MESSAGES = 500;
export function transcriptWindowReady(entries: readonly RuntimeClientTranscriptEntry[]) {
  const turns = new Set(entries.flatMap(entry => entry.turn_process ? [attemptOwnerKey(entry.turn_process)] : []));
  const messages = entries.filter(entry => entry.item.type === 'publication_audit' || entry.item.type === 'message'
    && (entry.item.message.role === 'assistant' || entry.item.message.role === 'user' && (!entry.item.message.kind || entry.item.message.kind === 'message'))).length;
  return messages >= HISTORY_MAX_MESSAGES || messages >= HISTORY_MIN_MESSAGES && turns.size >= HISTORY_MIN_TURNS;
}
export function traceWindowReady(records: readonly TraceRecord[]) {
  const turns = new Set(records.flatMap(record => { const key = traceAttemptKey(record.location); return key ? [key] : []; }));
  const messages = records.filter(record => ['user', 'assistant'].includes(record.kind)).length;
  return messages >= HISTORY_MAX_MESSAGES || messages >= HISTORY_MIN_MESSAGES && turns.size >= HISTORY_MIN_TURNS;
}
