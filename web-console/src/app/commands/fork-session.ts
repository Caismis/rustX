/* Copyright (c) 2026 DeepSeek. MIT. Catalog fork navigation adapted from session-controller sessions/service.ts; see PROVENANCE.md. */
import type { AppServerClient } from '../../client/app-server';
import { messageDescriptor } from '../../../../protocol/app-server/message-descriptor';
import { message, displayText } from '../../locale/translation';
import { localeController } from '../../locale/controller';
import { translator } from '../../locale/translation';

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
      const receipt: { sessionId: string; warning?: string } = { sessionId: fork.session.id };
      if (fork.durability_diagnostic) receipt.warning = `Session ${receipt.sessionId} was created with durability uncertainty: ${fork.durability_diagnostic}. Inspect Sessions; do not repeat the mutation.`;
      return receipt;
    }
    const before = result.window.page.next_cursor;
    if (before == null) throw new Error(displayText(translator(localeController.getSnapshot().active), message('workspace:fork.no-completed-response')));
    const older = await client.request({ method: 'session/history', params: { session_id: sessionId, node_id: session.active_node, at: { type: 'older', before, cut }, limit: 32 } }, 'session_history', undefined, admitted);
    if (older.window.page.next_cursor != null && BigInt(older.window.page.next_cursor) >= BigInt(before)) throw new Error('Fork history did not advance.');
    result = older;
  }
}
