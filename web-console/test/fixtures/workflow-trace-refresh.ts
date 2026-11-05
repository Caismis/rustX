import type { TraceCache, TraceFieldReader } from '../../src/client/trace';
import { refreshTrace } from '../../src/client/trace';
import { workflowTraceFixture } from '../workflow-trace-fixture';

/** A native-sized snapshot refresh with independently released page boundaries. */
export function workflowTraceRefreshFixture() {
  const original = workflowTraceFixture(73), fresh = workflowTraceFixture(73);
  fresh.cut.revision = '501'; fresh.cut.through = '501';
  for (const root of fresh.roots) root.collections[0].revision = '501';
  const selected = fresh.nodes.get(fresh.roots[0].collections[0].domain)![60];
  const field = selected.output;
  if (!field || field.type !== 'value') throw Error('Expected an original Workflow output selector');
  const output = JSON.stringify({ value: 'refreshed-output-60', returned: null, port: 'next' });
  selected.output_preview = output;
  fresh.inputs.set(field.domain, output);
  field.encoded_bytes = String(new TextEncoder().encode(JSON.stringify(output)).length);
  const waiting = new Map<number, () => void>(), requested: number[] = [];
  const source = (revision: string) => {
    if (revision === original.cut.revision) return original;
    if (revision === fresh.cut.revision) return fresh;
    throw Error('Unknown Workflow fixture cut');
  };
  const reader: TraceFieldReader = {
    page: async (cut, domain, offset, current) => {
      const fixture = source(cut.revision);
      const page = await fixture.reader.page(cut, domain, offset, current);
      if (fixture === fresh && domain === fresh.roots[0].collections[0].domain) {
        requested.push(offset);
        if (offset > 0) await new Promise<void>(resolve => waiting.set(offset, resolve));
      }
      return page;
    },
    value: (cut, ...args) => source(cut.revision).reader.value(cut, ...args),
    content: (cut, ...args) => source(cut.revision).reader.content(cut, ...args),
  };
  return {
    cache: { ...original.cache, fields: reader },
    refresh: (cache: TraceCache) => ({ ...refreshTrace(cache, { ...fresh.cache.page, read_cut: fresh.cut }), fields: reader }),
    controls: {
      requested: () => [...requested],
      release: (offset: number) => {
        const resolve = waiting.get(offset);
        if (!resolve) throw Error('Workflow page has not reached its gate');
        waiting.delete(offset); resolve();
      },
    },
  };
}

declare global { interface Window { workflowTraceRefresh: ReturnType<typeof workflowTraceRefreshFixture>['controls'] } }
