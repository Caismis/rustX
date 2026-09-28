// Regressions for fields contributed by draft-2020-12 $ref siblings.
// These must be usable through the public union, not a handwritten DTO.
import type {MessageBlock, UserContentBlock, RuntimeClientSnapshot} from './v28.js';

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

// Native response projections and explicit cut side are the mandatory v28 contract.
import type {CompletedResponseView, Request1} from './v28.js';
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

// Job discovery must expose omitted matches, and abandonment is a typed failure.
import type { MethodResult, ErrorData } from './v28.js';
const jobs = { type: 'jobs', jobs: [], returned: 0, matched: 0, limit: 64, truncated: false } satisfies MethodResult;
// @ts-expect-error A bounded list cannot silently omit its discovery metadata.
const dishonestJobs: MethodResult = { type: 'jobs', jobs: [] };
const failedJob = { kind: 'job_publication_abandoned', job_id: 'job-a' } satisfies ErrorData;
void jobs; void dishonestJobs; void failedJob;
// Creation intent already belongs to SessionPersistentState in mandatory v28.
// A display-only projected default must be expressible as no explicit override.
import type {SessionModelConfig} from './v28.js';
const initialModel = {model: 'provider/selected', reasoningProfile: 'high'} satisfies SessionModelConfig;
const explicitCreate = {method: 'session/create', params: {settings: {cwd: '/workspace', model: initialModel}}} satisfies Request1;
const nativeDefaultCreate = {method: 'session/create', params: {settings: {cwd: '/workspace'}}} satisfies Request1;
void explicitCreate;
void nativeDefaultCreate;

// One canonical generation includes both mainline catalog invalidation and Jobs/Agents.
import type { Notification } from './v28.js';
const summaryOnly = { jsonrpc: '2.0', method: 'session/summaryInvalidated', params: { session_id: 'session', catalog_changed: false } } satisfies Notification;
const membershipChanged = { ...summaryOnly, params: { session_id: 'session', catalog_changed: true } } satisfies Notification;
// @ts-expect-error Membership scope cannot be inferred from an older missing-flag shape.
const missingCatalogScope: Notification = { jsonrpc: '2.0', method: 'session/summaryInvalidated', params: { session_id: 'session' } };
void summaryOnly; void membershipChanged; void missingCatalogScope;

// Absence is a native unavailable fact in the mandatory vocabulary, not a decoder fallback.
import type { TraceToolSummary } from './v28.js';
const boundedToolArguments: TraceToolSummary['arguments'] = { text: '{"command":"pwd"}', truncated: false };
const unavailableToolArguments: TraceToolSummary['arguments'] = null;
void boundedToolArguments; void unavailableToolArguments;
