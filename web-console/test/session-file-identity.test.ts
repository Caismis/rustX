import { expect, it } from 'vitest';
import { sameSessionFile } from '../shared/session-file-identity.ts';
import type { SessionFileReference } from '../../protocol/app-server/v35.ts';
const file: SessionFileReference = { scope: { conversation_id: 'c', device: '1', inode: '2' }, path: 'a/report.xlsx', name: '报告.xlsx', mime_type: 'application/xlsx', description: 'report' };
it('compares typed identity independent of construction order; absent description is null', () => {
  expect(sameSessionFile(file, { description: 'report', mime_type: file.mime_type, name: file.name, path: file.path, scope: { inode: '2', device: '1', conversation_id: 'c' } })).toBe(true);
  expect(sameSessionFile({ ...file, description: undefined }, { ...file, description: null })).toBe(true);
  expect(sameSessionFile(undefined, file)).toBe(false);
  expect(sameSessionFile(undefined, undefined)).toBe(true);
});
it('detects every reference identity field change without granting read authority', () => {
  for (const key of ['path', 'name', 'mime_type', 'description'] as const) expect(sameSessionFile(file, { ...file, [key]: 'different' })).toBe(false);
  for (const key of ['conversation_id', 'device', 'inode'] as const) expect(sameSessionFile(file, { ...file, scope: { ...file.scope, [key]: 'different' } })).toBe(false);
});
