import { useEffect, useMemo, useRef, useState } from 'react';
import { validateCanonicalInspectionPage } from '../../../../protocol/app-server/trace-inspection';
import type { TraceCache } from '../../client/trace';
import type { TraceRecord } from '../../../../protocol/app-server/v76';
import { workflowCollection, type WorkflowNodes, type WorkflowTraceNode } from './workflow';

/** All admitted visits, including loop bodies, paged at the original native cut. */
export function useWorkflowNodes(cache: TraceCache, roots: readonly TraceRecord[], opened: ReadonlySet<string>) {
  const [retries, setRetries] = useState<Record<string, number>>({});
  const selected = roots.filter(root => opened.has(root.id));
  const keys = selected.map(root => [root.id, JSON.stringify([cache.epoch, root.id, cache.readCuts?.[root.id], workflowCollection(root), retries[root.id] ?? 0])] as const);
  const signature = JSON.stringify(keys), reader = cache.fields;
  const latest = useRef({ signature, reader }); latest.current = { signature, reader };
  const [state, setState] = useState<Record<string, { key: string; epoch: number; value: WorkflowNodes }>>({});
  useEffect(() => {
    let mounted = true;
    const current = () => mounted && latest.current.signature === signature && latest.current.reader === reader;
    setState(previous => Object.fromEntries(Object.entries(previous).filter(([id]) => selected.some(root => root.id === id))));
    for (const root of selected) {
      const key = keys.find(([id]) => id === root.id)![1], entry = state[root.id];
      const previous = entry?.epoch === cache.epoch && entry.value.reader === reader ? entry : undefined;
      if (previous?.key === key && !previous.value.error && !previous.value.loading && previous.value.reader === reader) continue;
      const cut = cache.readCuts?.[root.id], collection = workflowCollection(root);
      if (!reader || !cut || !collection) continue;
      const publish = (nodes: WorkflowTraceNode[], loading: boolean, error?: string) => {
        if (current()) setState(old => ({ ...old, [root.id]: { key, epoch: cache.epoch, value: { root, nodes, cut, reader, loading, error } } }));
      };
      const retain = (loading: boolean, error?: string) => {
        if (!previous) { publish([], loading, error); return; }
        // A refresh keeps the complete admitted snapshot and its own sources.
        // An old node's selectors must never be read at the replacement cut.
        if (current()) setState(old => ({ ...old, [root.id]: { key, epoch: cache.epoch, value: { ...previous.value, loading, error } } }));
      };
      void (async () => {
        const nodes: WorkflowTraceNode[] = [], ids = new Set<string>();
        let offset: number | null = collection.total === '0' ? null : 0;
        retain(offset !== null);
        while (offset !== null) {
          const page = await reader.page(cut, collection.domain, offset, current);
          if (!current()) return;
          if (page.domain !== collection.domain || JSON.stringify(page.read_cut) !== JSON.stringify(cut) || page.total !== collection.total) throw Error('Original Workflow node page changed.');
          validateCanonicalInspectionPage(page, offset, collection.total, true);
          for (const item of page.items) {
            const node = item.value as WorkflowTraceNode | null;
            if (!node || typeof node.id !== 'string' || ids.has(node.id) || !node.instance || typeof node.instance.node !== 'string' || !node.state || !node.timing) throw Error('Original Workflow node header is unavailable.');
            ids.add(node.id); nodes.push(node);
          }
          offset = page.next_offset == null ? null : Number(page.next_offset);
        }
        // Publish a cut atomically: a partial prefix would remove selected
        // visits on later pages and cause selection to migrate to a sibling.
        publish(nodes, false);
      })().catch(error => retain(false, String(error).slice(0, 512)));
    }
    return () => { mounted = false; };
  }, [signature, reader]);
  const nodes = useMemo(() => new Map(selected.flatMap(root => {
    const value = state[root.id]?.epoch === cache.epoch ? state[root.id]?.value : undefined;
    return value && value.reader === reader ? [[root.id, value] as const] : [];
  })), [state, signature, reader]);
  return { nodes, retry: (id: string) => {
    // A retired error control cannot request work from a replacement reader.
    if (latest.current.signature !== signature || latest.current.reader !== reader || !nodes.get(id)?.error) return;
    setRetries(previous => ({ ...previous, [id]: (previous[id] ?? 0) + 1 }));
  } };
}
