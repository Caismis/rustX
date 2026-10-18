import type { Page } from '@playwright/test';

type ObjectValue = Record<string, unknown>;
const object = (value: unknown): ObjectValue | undefined => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as ObjectValue : undefined;
const scalar = (value: unknown) => typeof value === 'number' || typeof value === 'boolean' || value === null ? value : typeof value === 'string' ? value.length <= 256 ? value : '<oversized diagnostic scalar>' : undefined;
const coordinates = (value: unknown, fields: readonly string[]) => {
  const source = object(value);
  return source && Object.fromEntries(fields.map(key => [key, scalar(source[key])]));
};
export function diagnosticCut(value: unknown) {
  const cut = object(value);
  if (!cut) return;
  const source = object(cut.source_cut);
  return {
    ...coordinates(cut, ['conversation_id', 'activation_id', 'revision', 'through']),
    source: source && { ...coordinates(source, ['ledger_through', 'surface_revision', 'transcript_total', 'inherited_through']), read: coordinates(source.read, ['conversation_id', 'journal', 'ledger_through', 'surface_revision', 'transcript_total', 'transcript', 'mutation_revision', 'pending_through', 'pending_total']) },
  };
}

/** Failure-only scalar evidence. Observes real frames without routing or retaining bodies. */
export function wireDiagnostics(page: Page) {
  const start = Date.now();
  const rows: unknown[] = [];
  let sockets = 0;
  const append = (row: object) => {
    rows.push({ ms: Date.now() - start, ...row });
    if (rows.length > 64) rows.shift();
  };
  page.on('websocket', socket => {
    const owner = ++sockets;
    const methods = new Map<string, string>();
    const frame = (direction: 'out' | 'in', payload: string | Buffer) => {
      const text = typeof payload === 'string' ? payload : payload.toString('utf8');
      if (Buffer.byteLength(text) > 1_048_576) { append({ owner, direction, oversized: true }); return; }
      let parsed: unknown;
      try { parsed = JSON.parse(text); } catch { append({ owner, direction, malformed: true }); return; }
      const value = object(parsed);
      if (!value) { append({ owner, direction, malformed: true }); return; }
      const id = scalar(value.id), params = object(value.params), result = object(value.result), error = object(value.error);
      const method = typeof value.method === 'string' ? value.method : methods.get(String(id));
      if (direction === 'out' && method && id !== undefined) {
        if (methods.size === 64) methods.delete(methods.keys().next().value!);
        methods.set(String(id), method);
      } else if (direction === 'in' && id !== undefined) methods.delete(String(id));
      const event = object(params?.event), metadata = object(event?.metadata), checkpoint = object(result?.checkpoint), eventCheckpoint = object(event?.checkpoint);
      append({ owner, direction, id, method, result: scalar(result?.type), error: error && { code: scalar(error.code), kind: scalar(object(error.data)?.kind) },
        target: coordinates(params?.target ?? result?.target, ['session_id', 'conversation_id', 'runtime_incarnation', 'attachment_id']),
        registration_id: scalar(params?.registration_id ?? result?.registration_id), cursor: scalar(params?.cursor ?? checkpoint?.cursor), after_cursor: scalar(params?.after_cursor ?? result?.after_cursor),
        event: scalar(event?.type), facts: Array.isArray(event?.events) ? event.events.slice(0, 8).map(fact => scalar(object(fact)?.type)) : undefined,
        cut: diagnosticCut(checkpoint?.read_cut ?? metadata?.read_cut ?? eventCheckpoint?.read_cut ?? object(result?.page)?.read_cut),
      });
    };
    socket.on('framesent', ({ payload }) => frame('out', payload));
    socket.on('framereceived', ({ payload }) => frame('in', payload));
    socket.on('close', () => append({ owner, closed: true }));
    socket.on('socketerror', message => append({ owner, socket_error: message.slice(0, 256) }));
  });
  return () => ({ sockets, rows });
}
