import { translator } from '../src/locale/translation';
import { describe, expect, it } from 'vitest';
import { composerSubmissionPolicy, type ComposerSubmissionFacts, type SubmitGesture } from '../src/app/composer/submission-policy';
const ready: ComposerSubmissionFacts = { running: false, busyEnter: 'queue', actionable: true, draftKind: 'message', blocked: false, acknowledging: false, uploadsPending: false, cancellationAvailable: true };
describe('composer submission policy (no runtime authority)', () => {
  it.each<[string, Partial<ComposerSubmissionFacts>, SubmitGesture, string, string, boolean, string | undefined, string]>([
    ['idle Enter', {}, 'enter', 'message', 'Send', false, 'send', 'send'],
    ['idle accelerated', { busyEnter: 'steer' }, 'accelerated', 'message', 'Send', false, 'send', 'send'],
    ['idle steer preference Enter', { busyEnter: 'steer' }, 'enter', 'message', 'Send', false, 'send', 'send'],
    ['idle empty', { actionable: false }, 'enter', 'message', 'Send', true, 'send', 'send'],
    ['running empty', { running: true, actionable: false }, 'enter', 'stop', 'Stop', false, undefined, 'stop-double-escape'],
    ['running empty cannot cancel', { running: true, actionable: false, cancellationAvailable: false }, 'enter', 'stop', 'Stop', true, undefined, 'stop-double-escape'],
    ['running blocked', { running: true, blocked: true }, 'enter', 'stop', 'Stop', false, undefined, 'stop-double-escape'],
    ['running blocked cannot cancel', { running: true, blocked: true, cancellationAvailable: false }, 'enter', 'stop', 'Stop', true, undefined, 'stop-double-escape'],
    ['running queue Enter', { running: true }, 'enter', 'message', 'Queue', false, 'send', 'queue-enter-ctrl-cmd-enter-to-steer'],
    ['running queue accelerated', { running: true }, 'accelerated', 'message', 'Steer', false, 'steer', 'steer'],
    ['running steer Enter', { running: true, busyEnter: 'steer' }, 'enter', 'message', 'Steer', false, 'steer', 'steer-enter-ctrl-cmd-enter-to-queue'],
    ['running steer accelerated', { running: true, busyEnter: 'steer' }, 'accelerated', 'message', 'Queue', false, 'send', 'queue'],
    ['cancellation pending still accepts draft', { running: true, cancellationAvailable: false }, 'enter', 'message', 'Queue', false, 'send', 'queue-enter-ctrl-cmd-enter-to-steer'],
    ['running uploads pending', { running: true, uploadsPending: true, busyEnter: 'steer' }, 'enter', 'message', 'Send', true, 'steer', 'resolve-draft-uploads-before-sending'],
    ['idle uploads pending', { uploadsPending: true }, 'enter', 'message', 'Send', true, 'send', 'resolve-draft-uploads-before-sending'],
    ['running acknowledging', { running: true, acknowledging: true }, 'enter', 'message', 'Send', true, 'send', 'awaiting-acknowledgement'],
    ['idle acknowledging', { acknowledging: true }, 'enter', 'message', 'Send', true, 'send', 'awaiting-acknowledgement'],
    ['idle blocked', { blocked: true }, 'enter', 'message', 'Send', true, 'send', 'send'],
    ['idle command', { draftKind: 'command' }, 'enter', 'command', 'Run command', false, undefined, 'run-command'],
    ['running command', { running: true, busyEnter: 'steer', draftKind: 'command' }, 'enter', 'command', 'Run command', false, undefined, 'run-command'],
    ['accelerated command', { running: true, busyEnter: 'steer', draftKind: 'command' }, 'accelerated', 'command', 'Run command', false, undefined, 'run-command'],
    ['unsupported slash', { running: true, busyEnter: 'steer', draftKind: 'unsupported-command' }, 'enter', 'command', 'Review command', false, undefined, 'review-command'],
    ['command upload gate', { draftKind: 'command', uploadsPending: true }, 'enter', 'command', 'Run command', true, undefined, 'run-command'],
    ['command ack gate', { draftKind: 'command', acknowledging: true }, 'enter', 'command', 'Run command', true, undefined, 'run-command'],
  ])('%s', (_, patch, gesture, kind, label, disabled, delivery, title) => {
    const result = composerSubmissionPolicy({ ...ready, ...patch }, gesture);
    expect(result.kind).toBe(kind); expect(translator('en')(result.label)).toBe(label);
    expect(result.disabled).toBe(disabled); expect(result.title).toBe(`agent:submission.${title}`);
    expect('delivery' in result ? result.delivery : undefined).toBe(delivery);
  });
});
