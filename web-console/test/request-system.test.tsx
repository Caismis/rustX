import { expect, it } from 'vitest';
import type { TraceToolDefinition } from '../../protocol/app-server/v80';
import { completeTools } from '../src/client/request-system';
import { readRequestField } from '../src/client/trace-content';
import { traceRecord, requestDetail } from './trace-fixture';
import { nativeTraceFixture } from './native-trace-fixture';

it('reads the original Tool catalog through the preceding request field', async () => {
  const detail = requestDetail(2);
  const { original, page } = nativeTraceFixture(traceRecord(2), detail);
  const previous = 'trace.records.4.detail.request.previous_tools';
  const native = 'trace.records.2.detail.request.tools';
  const reader = {
    ...original,
    page: async (...[cut, domain, offset, current]: Parameters<typeof original.page>) => ({
      ...original.page(cut, native, offset, current), domain,
    }),
    value: async (...[cut, field, offset, current]: Parameters<typeof original.value>) => ({
      ...original.value(cut, { ...field, domain: native }, offset, current), domain: field.domain,
    }),
    content: async (...args: Parameters<typeof original.content>) => original.content(...args),
  };
  await expect(readRequestField(reader, page.read_cut, previous, () => true)).resolves.toEqual(detail.request!.tools);
});

it('rejects partial Tool catalogs', () => {
  const original = requestDetail(2).request!.tools, updated: TraceToolDefinition[] = [{ ...original[0]!, description: { text: 'New instructions', truncated: false } }];
  expect(completeTools(updated)).toBe(true);
  expect(completeTools([{ ...updated[0]!, input_schema: { value: {}, truncated: true } }])).toBe(false);
});
