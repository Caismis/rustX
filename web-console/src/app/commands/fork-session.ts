/* Copyright (c) 2026 DeepSeek. MIT. Catalog fork navigation adapted from session-controller sessions/service.ts; see PROVENANCE.md. */
import type { AppServerClient } from '../../client/app-server';
import type { MethodResult } from '../../../../protocol/app-server/v59';
import { messageDescriptor } from '../../../../protocol/app-server/message-descriptor';
import { message, displayText } from '../../locale/translation';
import { localeController } from '../../locale/controller';
import { translator } from '../../locale/translation';

/** Increment a durable fork title, preserving half/full-width parentheses. */
export function increasedForkTitle(title: string): string {
  const ascii = /^(.*?)\((\d+)\)$/u.exec(title);
  const fullWidth = /^(.*?)（(\d+)）$/u.exec(title);
  const numbered = ascii ?? fullWidth;
  const suffix = numbered ? `${ascii ? '(' : '（'}${BigInt(numbered[2]!) + 1n}${ascii ? ')' : '）'}` : ' (1)';
  // Rust validates Unicode scalar values, rather than UTF-16 code units.
  const tail = Array.from(suffix).length <= 120 ? suffix : ' (1)';
  const stem = numbered && tail === suffix ? numbered[1]! : title;
  return Array.from(stem).slice(0, 120 - Array.from(tail).length).join('') + tail;
}

/** Finish committed Fork metadata independently of its disposable navigation.
 * Every Web Fork entry increments the native name or first-message title. */
export async function finishForkSession(client: AppServerClient, fork: Extract<MethodResult, { type: 'session_transition' }>, sourceTitle: string | null | undefined, generation: number): Promise<string | undefined> {
  const current = () => client.getSnapshot().generation === generation && client.getSnapshot().connection === 'connected';
  const refresh = () => { if (current()) void client.listSessions().catch(() => {}); };
  refresh();
  if (fork.durability_diagnostic) return `Session ${fork.session.id} was created with durability uncertainty: ${fork.durability_diagnostic}. Inspect Sessions; do not repeat the mutation.`;
  if (sourceTitle == null) return;
  let warning: string | undefined;
  try {
    if (!current()) throw new Error('Connection changed before automatic naming.');
    await client.renameSession(fork.session.id, increasedForkTitle(sourceTitle));
  } catch (cause) {
    warning = `Session ${fork.session.id} was created, but automatic naming failed: ${String(cause)}. Open the created Session to rename it; do not repeat the fork.`;
  }
  refresh();
  return warning;
}

/** Sidebar Fork resolves the last native completion without selecting or
 * attaching its source. The existing explicit /fork boundary picker is separate. */
export async function forkLatestSession(client: AppServerClient, sessionId: string, current: () => boolean) {
  const generation = client.getSnapshot().generation;
  const admitted = await client.admitSessionOperation(sessionId, current);
  if (!admitted || !current()) return;
  const { session } = await client.request({ method: 'session/read', params: { session_id: sessionId } }, 'session', undefined, admitted);
  if (!current() || !admitted.current()) return;
  const state = client.getSnapshot();
  const summary = state.sessions.find(row => row.id === sessionId) ?? state.views[sessionId]?.summary;
  const sourceTitle = session.name ?? session.generated_title?.text ?? summary?.name ?? summary?.generated_title?.text ?? summary?.preview;
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
      const receipt: { sessionId: string; warning?: string } = { sessionId: fork.session.id };
      const warning = await finishForkSession(client, fork, sourceTitle, generation);
      if (warning) receipt.warning = warning;
      return receipt;
    }
    const before = result.window.page.next_cursor;
    if (before == null) throw new Error(displayText(translator(localeController.getSnapshot().active), message('workspace:fork.no-completed-response')));
    const older = await client.request({ method: 'session/history', params: { session_id: sessionId, node_id: session.active_node, at: { type: 'older', before, cut }, limit: 32 } }, 'session_history', undefined, admitted);
    if (older.window.page.next_cursor != null && BigInt(older.window.page.next_cursor) >= BigInt(before)) throw new Error('Fork history did not advance.');
    result = older;
  }
}
