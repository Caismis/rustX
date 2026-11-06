import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from 'react';
import type { WorkflowToolSource, WorkflowToolCatalogCursor } from '../../../../../../protocol/app-server/v73';
import type { WorkflowScopePort } from '../../port';

export type ToolCatalogState = { status: 'idle' | 'loading' | 'failed' } | { status: 'ready'; sources: readonly WorkflowToolSource[] };

const Catalog = createContext<{ state: ToolCatalogState; load: () => void }>({ state: { status: 'idle' }, load: () => {} });

/** Each page belongs to one unchanged catalog. A source becomes listed only
 * after every page has arrived through the same captured authority. */
export async function readToolCatalog(port: Pick<WorkflowScopePort, 'toolCatalog'>, source?: string) {
  const sources: WorkflowToolSource[] = [];
  let cursor: WorkflowToolCatalogCursor | undefined;
  do {
    const catalog = await port.toolCatalog(source, cursor);
    for (const item of catalog.sources) {
      const prior = source && sources.find(prior => prior.source_id === item.source_id);
      if (prior) prior.tools.push(...item.tools);
      else sources.push({ ...item, tools: [...item.tools] });
    }
    const next = catalog.next ?? undefined;
    if (next && cursor && (next.offset <= cursor.offset || next.digest !== cursor.digest)) throw new Error('Invalid Tool catalog continuation');
    cursor = next;
  } while (cursor);
  return { sources };
}

/** The scope's MCP servers and Managed Python packages, read once per
 * editor when first needed, then each listed on its own, so a package still
 * preparing its environment delays only itself. */
export function ToolCatalogProvider({ port, children }: { port: WorkflowScopePort; children: ReactNode }) {
  const [state, setState] = useState<ToolCatalogState>({ status: 'idle' }), pending = useRef(false);
  const load = useCallback(() => {
    if (pending.current) return;
    pending.current = true;
    setState({ status: 'loading' });
    const settle = (source: string, next: (current: WorkflowToolSource) => WorkflowToolSource) => setState(current => current.status !== 'ready' ? current
      : { status: 'ready', sources: current.sources.map(item => item.source_id === source ? next(item) : item) });
    readToolCatalog(port).then(catalog => {
      setState({ status: 'ready', sources: catalog.sources });
      for (const { source_id, state } of catalog.sources) if (state === 'pending') {
        readToolCatalog(port, source_id).then(
          listed => { const found = listed.sources.find(item => item.source_id === source_id); settle(source_id, current => found ?? { ...current, state: 'list_failed' }); },
          () => settle(source_id, current => ({ ...current, state: 'list_failed' })));
      }
    }, () => {
      pending.current = false;
      setState({ status: 'failed' });
    });
  }, [port]);
  const value = useMemo(() => ({ state, load }), [state, load]);
  return <Catalog.Provider value={value}>{children}</Catalog.Provider>;
}

export const useToolCatalog = () => useContext(Catalog);
