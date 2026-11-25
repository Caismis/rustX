import { createContext, useCallback, useContext, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import type { WorkflowToolSource } from '../../../../../../protocol/app-server/v81';
import { readToolCatalog, type WorkflowScopePort } from '../../port';

export type ToolCatalogState = { status: 'idle' | 'loading' | 'failed' } | { status: 'ready'; sources: readonly WorkflowToolSource[] };

const Catalog = createContext<{ state: ToolCatalogState; load: () => void }>({ state: { status: 'idle' }, load: () => {} });

/** Load on first use. Workspace listings share one HTTP request at a time
 * because each Product Host request takes a dedicated native connection. */
export function ToolCatalogProvider({ port, children }: { port: WorkflowScopePort; children: ReactNode }) {
  const [state, setState] = useState<ToolCatalogState>({ status: 'idle' });
  const loading = useRef<AbortController | undefined>(undefined);
  useLayoutEffect(() => () => {
    loading.current?.abort();
    loading.current = undefined;
  }, [port]);
  const load = useCallback(() => {
    if (loading.current) return;
    const abort = new AbortController();
    const { signal } = abort;
    loading.current = abort;
    setState({ status: 'loading' });
    const settle = (source: string, listed?: WorkflowToolSource) => {
      if (signal.aborted) return;
      setState(current => current.status !== 'ready' ? current : { status: 'ready',
        sources: current.sources.map(item => item.source_id === source ? listed ?? { ...item, state: 'list_failed' } : item) });
    };
    const list = async (id: string) => {
      try {
        const listed = await readToolCatalog(port, id, undefined, { signal });
        settle(id, listed.sources.find(item => item.source_id === id));
      } catch { settle(id); }
    };
    void readToolCatalog(port, undefined, undefined, { signal }).then(async catalog => {
      if (signal.aborted) return;
      setState({ status: 'ready', sources: catalog.sources });
      for (const { source_id, state } of catalog.sources) {
        if (signal.aborted) break;
        if (state !== 'pending') continue;
        if (port.scope.kind === 'workspace') await list(source_id);
        else void list(source_id);
      }
    }, () => {
      if (signal.aborted) return;
      loading.current = undefined;
      setState({ status: 'failed' });
    });
  }, [port]);
  const value = useMemo(() => ({ state, load }), [state, load]);
  return <Catalog.Provider value={value}>{children}</Catalog.Provider>;
}

export const useToolCatalog = () => useContext(Catalog);
