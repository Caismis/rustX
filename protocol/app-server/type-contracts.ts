// Regressions for fields contributed by draft-2020-12 $ref siblings.
// These must be usable through the public union, not a handwritten DTO.
import type {MessageBlock, UserContentBlock, RuntimeClientSnapshot} from './v10.js';

const text = {type: 'text', text: 'Native content'} satisfies UserContentBlock;
const user = {role: 'user', id: 'user-1', content: [text], source: 'human'} satisfies MessageBlock;
const assistant = {role: 'assistant', id: 'assistant-1', content: [text]} satisfies MessageBlock;
const read = (message: MessageBlock): string => message.id;
read(user);
read(assistant);
// @ts-expect-error Canonical messages require their Rust-owned identity/content.
const incomplete: MessageBlock = {role: 'assistant'};
void incomplete;

// A numeric Rust default annotation must not narrow an exact string-domain ref.
const revision: RuntimeClientSnapshot['capabilities']['revision'] = '9007199254740993';
// @ts-expect-error Public exact revisions never become JSON numbers.
const numericRevision: RuntimeClientSnapshot['capabilities']['revision'] = 0;
void revision;
void numericRevision;

// Tool results carry native ownership independently of provider correlation.
const tool = {
  role: 'tool', id: 'result-B',
  occurrence: {assistant_message_id: 'assistant-B', block_index: 2},
  tool_call_id: 'call_1', tool_id: 'tool-bash',
  result: {status: {type: 'success'}, content: [], duration_ms: 0},
} satisfies MessageBlock;
read(tool);
const {occurrence: nativeOwner, ...missingOwner} = tool;
// @ts-expect-error Provider call IDs alone cannot identify a canonical result owner.
const invalidTool: MessageBlock = missingOwner;
void nativeOwner;
void invalidTool;

// Native response projections and explicit cut side are the mandatory v10 contract.
import type {CompletedResponseView, Request1} from './v10.js';
const completed = {
  closing_message_id: 'destination-assistant',
  origin: {conversation_id: 'source-conversation', attempt_id: 'source-attempt', closing_message_id: 'source-assistant'},
  completed_at: '2026-09-19T00:00:00Z', surface_revision: '42',
  timing: {total_duration_ms: 19000, ttft_ms: 320, generation_ms: 1280, output_tokens_per_second: 15.625},
} satisfies CompletedResponseView;
void completed;
const after: Extract<Request1, {method: 'session/branch'}>['params']['side'] = 'after';
const before: Extract<Request1, {method: 'session/branch'}>['params']['side'] = 'before';
void after; void before;
