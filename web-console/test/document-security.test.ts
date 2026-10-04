// @vitest-environment node
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { expect, it, vi } from 'vitest';
const converter = vi.hoisted(() => vi.fn());
vi.mock('../host/documents/converter', () => ({ convertOffice: converter }));
import { deriveDocument } from '../host/documents/operation';
import { admitOoxml } from '../host/documents/archive';
import { zip } from './document-fixtures';

it.each(['docx', 'pptx', 'xlsx'] as const)('%s active or external package content fails in the real worker before conversion/publication', async extension => {
  const original = admitOoxml(readFileSync(new URL(`./fixtures/documents/sample.${extension}`, import.meta.url)));
  for (const extra of [
    { name: 'vbaProject.bin', data: 'executable macro payload' },
    { name: 'external.rels', data: '<Relationships><Relationship Id="x" Type="externalLink" TargetMode="External" Target="https://rustx-preview.invalid/tracker"/></Relationships>' },
    { name: 'oversized.xml', data: 'x', size: 4 * 1024 * 1024 + 1 },
  ]) {
    const bytes = zip([...original].map(([name, data]) => ({ name, data: data.toString() })).concat(extra));
    let reads = 0;
    await expect(deriveDocument({ extension, source: { kind: 'artifact', artifact_id: 'immutable-id' },
      target: { session_id: 's', conversation_id: 'c', runtime_incarnation: '1', attachment_id: 'a' },
      digest: createHash('sha256').update(bytes).digest('hex'),
    }, async () => { reads++; return { data: bytes.toString('base64') }; }, new AbortController().signal)).rejects.toThrow('archive_rejected');
    expect(reads).toBe(1); expect(converter).not.toHaveBeenCalled();
  }
});
