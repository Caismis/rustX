// Regressions for fields contributed by draft-2020-12 $ref siblings.
// These must be usable through the public union, not a handwritten DTO.
import type {MessageBlock, UserContentBlock, RuntimeClientSnapshot} from './v4.js';

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
