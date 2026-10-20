/* Copyright (c) 2026 DeepSeek. MIT. Fork-title numbering adapted from session-controller sessions/service.ts; see PROVENANCE.md. */
import type { AppServerClient } from '../../client/app-server';
import { messageDescriptor } from '../../../../protocol/app-server/message-descriptor';
import { message, displayText } from '../../locale/translation';
import { localeController } from '../../locale/controller';
import { translator } from '../../locale/translation';

/** Increment a durable fork title, preserving half/full-width parentheses. */
export function increasedForkTitle(title: string): string {
  const ascii = /^(.*?)\((\d+)\)$/u.exec(title);
  if (ascii?.[1] !== undefined && ascii[2] !== undefined) return `${ascii[1]}(${BigInt(ascii[2]) + 1n})`;
  const fullWidth = /^(.*?)（(\d+)）$/u.exec(title);
  if (fullWidth?.[1] !== undefined && fullWidth[2] !== undefined) return `${fullWidth[1]}（${BigInt(fullWidth[2]) + 1n}）`;
  return `${title} (1)`;
}

/** Sidebar Fork resolves the last native completion without selecting or
 * attaching its source. The existing explicit /fork boundary picker is separate. */
export async function forkLatestSession(client: AppServerClient, sessionId: string, current: () => boolean) {
  const admitted = await client.admitSessionOperation(sessionId, current);
  if (!admitted || !current()) return;
  const { session } = await client.request({ method: 'session/read', params: { session_id: sessionId } }, 'session', undefined, admitted);
  if (!current() || !admitted.current()) return;
  let result = await client.request({ method: 'session/history', params: { session_id: sessionId, node_id: session.active_node, at: { type: 'latest' }, limit: 32 } }, 'session_history', undefined, admitted);
  const cut = result.window.cut;
  while (current() && admitted.current()) {
    if (result.conversation_id !== session.active_conversation_id || result.window.cut.conversation_id !== cut.conversation_id) throw new Error('Fork history belongs to a different Conversation.');
    // Large bodies can be deferred while their native completion stays in the
    // descriptor. Compare ledger cursors across both representations.
    const completions = [
      ...(result.window.page.entries ?? []).map(entry => ({ cursor: entry.cursor, response: entry.completed_response })),
      ...(result.window.page.deferred_entries ?? []).map(entry => ({ cursor: entry.offset, response: messageDescriptor(entry.value)?.completed_response })),
    ];
    const latest = completions.reduce<typeof completions[number] | undefined>((selected, entry) =>
      entry.response && (!selected || BigInt(entry.cursor) > BigInt(selected.cursor)) ? entry : selected, undefined);
    const response = latest?.response;
    if (response) {
      const fork = await client.request({ method: 'session/fork', params: { session_id: sessionId, node_id: session.active_node,
        surface_revision: response.surface_revision, boundary: response.closing_locator, side: 'after' } }, 'session_transition', undefined, admitted);
      // A dispatched native mutation settles even if its sidebar gesture retires.
      void client.listSessions().catch(() => {});
      if (!current()) return;
      if (fork.durability_diagnostic) throw new Error(`Fork committed with durability uncertainty: ${fork.durability_diagnostic}. Inspect Sessions; do not repeat the mutation.`);
      if (session.name) await client.renameSession(fork.session.id, increasedForkTitle(session.name));
      if (!current()) return;
      void client.listSessions().catch(() => {});
      return fork.session.id;
    }
    const before = result.window.page.next_cursor;
    if (before == null) throw new Error(displayText(translator(localeController.getSnapshot().active), message('workspace:fork.no-completed-response')));
    const older = await client.request({ method: 'session/history', params: { session_id: sessionId, node_id: session.active_node, at: { type: 'older', before, cut }, limit: 32 } }, 'session_history', undefined, admitted);
    if (older.window.page.next_cursor != null && BigInt(older.window.page.next_cursor) >= BigInt(before)) throw new Error('Fork history did not advance.');
    result = older;
  }
}
