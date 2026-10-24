import { expect, it } from 'vitest';
import type { BoundedSourceSettings, SourceFieldRef } from '../../protocol/app-server/v57';
import { readSourceRepairFields } from '../../protocol/app-server/source-observation';

it('repair recovery reads only its advertised fields and retains the exact native sequence', async () => {
  const json = '"9007199254740993"';
  const reference: SourceFieldRef = { revision: 'captured', field: 'repair_sequence', byte_length: new TextEncoder().encode(json).length };
  const view: BoundedSourceSettings = { target: { kind: 'workspace', directory: '/workspace/A' }, revision: 'captured', inline: null, collections: [], fields: [
    { name: 'repair_sequence', reference },
    { name: 'user_repair', value: null },
    { name: 'workspace_repair', value: null },
    { name: 'user', reference: { revision: 'captured', field: 'user', byte_length: 20 * 1024 * 1024 } },
  ] };
  const reads: SourceFieldRef[] = [];
  const result = await readSourceRepairFields(view, async (selected, offset) => {
    reads.push(selected);
    expect(offset).toBe(0);
    expect(selected).toEqual(reference);
    return { reference: selected, offset, end_offset: reference.byte_length, data_base64: btoa(json), next_offset: null };
  }, () => true);
  expect(reads).toEqual([reference]);
  expect(result).toEqual({ target: view.target, repair_sequence: '9007199254740993', user_repair: null, workspace_repair: null });
});

it('a repair field read retired by a connection replacement cannot return an observation', async () => {
  const reference: SourceFieldRef = { revision: 'captured', field: 'repair_sequence', byte_length: 3 };
  let current = true;
  await expect(readSourceRepairFields({ target: { kind: 'user' }, revision: 'captured', inline: null, collections: [], fields: [{ name: 'repair_sequence', reference }] }, async () => {
    current = false;
    return { reference, offset: 0, end_offset: 3, data_base64: btoa('"1"'), next_offset: null };
  }, () => current)).rejects.toThrow('replaced');
});
