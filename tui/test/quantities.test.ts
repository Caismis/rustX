import assert from 'node:assert/strict';
import { test } from 'node:test';
import { compactQuantity, groupedQuantity, unsignedQuantity } from '../src/protocol/quantities.ts';
import { formatDuration } from '../src/ui/components/tool-card.ts';

test('T08 T14 native unsigned quantities retain u64 precision in terminal formatting', () => {
  assert.equal(unsignedQuantity('18446744073709551615'), 18446744073709551615n);
  assert.equal(groupedQuantity('9007199254740993'), '9,007,199,254,740,993');
  assert.equal(compactQuantity('9007199254740993'), '9007199254741k');
  assert.equal(compactQuantity('12500'), '12.5k');
  assert.equal(formatDuration('18446744073709551615'), '307445734561825m52s');
  assert.equal(formatDuration('119999'), '2m00s');
  assert.equal(formatDuration('12'), '12ms');
  assert.throws(() => unsignedQuantity(9007199254740992));
});
