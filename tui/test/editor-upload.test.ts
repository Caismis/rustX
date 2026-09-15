import assert from 'node:assert/strict';
import { it } from 'node:test';
import { editorText, editorSubmission } from '../src/app-server/editor.ts';
import type { UserInputBlock } from '../src/protocol/app-server.ts';
it('restored drafts preserve ordered receipts and exact text through submission', () => {
  const draft: UserInputBlock[] = [
    { type: 'upload', session_id: 'destination', batch_id: 'a', token: 'one' },
    { type: 'text', text: '\n body  \n' },
    { type: 'upload', session_id: 'destination', batch_id: 'b', token: 'two' },
  ];
  assert.equal(editorText(draft), '\n body  \n');
  assert.deepEqual(editorSubmission(draft, editorText(draft)), draft);
  assert.deepEqual(editorSubmission(draft, 'edited'), [draft[0], { type: 'text', text: 'edited' }, draft[2]]);
});
