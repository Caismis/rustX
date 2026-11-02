import { expect, it, vi } from 'vitest';
import type { WorkflowToolCatalogCursor, WorkflowToolSource } from '../../protocol/app-server/v63';
import { readToolCatalog } from '../src/app/workflows/ui/editor/toolCatalog';

it('collects every page of one source before exposing the complete catalog', async () => {
  const source: WorkflowToolSource = { source_id: 'large', kind: 'mcp', scope: 'user', state: 'listed', tools: Array.from({ length: 128 }, (_, index) => ({ name: `tool_${index}`, description: '', input_schema: { type: 'object' }, result: { type: 'text' } })) };
  const toolCatalog = vi.fn(async (_source?: string, cursor?: WorkflowToolCatalogCursor | null) => {
    const offset = cursor?.offset ?? 0;
    return { sources: [{ ...source, tools: source.tools.slice(offset, offset + 17) }], next: offset + 17 < source.tools.length ? { offset: offset + 17, digest: 'a'.repeat(64) } : null };
  });
  const result = await readToolCatalog({ toolCatalog }, 'large');
  expect(result.sources).toEqual([source]);
  expect(toolCatalog.mock.calls.map(([source, cursor]) => [source, cursor?.offset ?? 0])).toEqual([0, 17, 34, 51, 68, 85, 102, 119].map(offset => ['large', offset]));
});

it('rejects a changed or non-advancing continuation without exposing partial choices', async () => {
  for (const next of [{ offset: 17, digest: 'a'.repeat(64) }, { offset: 34, digest: 'b'.repeat(64) }]) {
    let calls = 0;
    await expect(readToolCatalog({ toolCatalog: async () => ({ sources: [], next: calls++ === 0 ? { offset: 17, digest: 'a'.repeat(64) } : next }) }, 'large')).rejects.toThrow('Invalid Tool catalog continuation');
    expect(calls).toBe(2);
  }
});
