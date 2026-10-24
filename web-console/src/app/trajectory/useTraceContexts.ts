import { useEffect, useMemo, useRef, useState } from 'react';
import type { TraceContextPresentation } from '../../../../protocol/app-server/v57';
import { canonicalLocatorKey } from '../../../../protocol/app-server/content-identity';
import { validateCanonicalInspectionPage } from '../../../../protocol/app-server/trace-inspection';
import { TRACE_DETAIL_LIMIT, type TraceCache } from '../../client/trace';

/** Context continuation adds normal ledger rows for the inspected request. */
export function useTraceContexts(cache: TraceCache, owner?: string) {
  const record = cache.page.records.find(record => record.id === owner), request = record?.request;
  const cut = record && cache.readCuts?.[record.id], reader = cache.fields;
  const identity = JSON.stringify([cache.epoch, record?.id, cut, request?.context_domain, request?.context_total]);
  const latest = useRef({ identity, reader }); latest.current = { identity, reader };
  const [state, setState] = useState<Record<string, { identity: string; rows?: TraceContextPresentation[]; error?: string }>>({});
  const loaded = record && state[record.id]?.identity === identity ? state[record.id] : undefined;
  const publish = (value: { rows?: TraceContextPresentation[]; error?: string }) => setState(previous => ({ ...Object.fromEntries(Object.entries(previous).filter(([id]) => id !== record!.id).slice(-(TRACE_DETAIL_LIMIT - 1))), [record!.id]: { identity, ...value } }));
  useEffect(() => {
    if (!reader || !request || !cut || request.context_next_offset == null || loaded?.rows) return;
    let mounted = true;
    const current = () => mounted && latest.current.identity === identity && latest.current.reader === reader;
    void (async () => {
      const rows: TraceContextPresentation[] = []; let offset: number | null | undefined = request.context_next_offset;
      while (offset != null) {
        const page = await reader.page(cut, request.context_domain, offset, current);
        if (!current()) return;
        if (!Array.isArray(page.items) || page.items.length > 32 || page.domain !== request.context_domain || page.cursor !== cut.revision || JSON.stringify(page.read_cut) !== JSON.stringify(cut)) throw Error('Original Context page changed.');
        validateCanonicalInspectionPage(page, offset, request.context_total, true);
        for (const item of page.items) {
          if (item.deferred || !item.value || typeof item.value !== 'object' || !('locator' in item.value) || !('fields' in item.value) || !Array.isArray(item.value.fields)) throw Error('Original Context header is unavailable.');
          const row = item.value as unknown as TraceContextPresentation;
          canonicalLocatorKey(row.locator); rows.push(row);
        }
        offset = page.next_offset == null ? undefined : Number(page.next_offset);
      }
      if (current()) publish({ rows });
    })().catch(error => { if (current()) publish({ error: String(error).slice(0, 512) }); });
    return () => { mounted = false; };
  }, [identity, reader]);
  const records = useMemo(() => cache.page.records.map(record => {
    const request = record.request, entry = state[record.id];
    if (!request || !entry?.rows || entry.identity !== JSON.stringify([cache.epoch, record.id, cache.readCuts?.[record.id], request.context_domain, request.context_total])) return record;
    return { ...record, request: { ...request, context_additions: [...request.context_additions, ...entry.rows], context_next_offset: null } };
  }), [cache.page.records, cache.epoch, cache.readCuts, state]);
  return { records, error: loaded?.error };
}
