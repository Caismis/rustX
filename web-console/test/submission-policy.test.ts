import { describe, expect, it } from 'vitest';
import { composerSubmissionPolicy, type ComposerSubmissionFacts, type SubmitGesture } from '../src/app/composer/submission-policy';
const ready: ComposerSubmissionFacts = { running: false, actionable: true, draftKind: 'message', blocked: false, acknowledging: false, uploadsPending: false, cancellationAvailable: true };
describe('composer submission policy (no runtime authority)', () => {
  it.each<[string, Partial<ComposerSubmissionFacts>, SubmitGesture, string, boolean, string | undefined]>([
    ['idle text', {}, 'enter', 'Send', false, 'send'],
    ['idle accelerated text', {}, 'accelerated', 'Send', false, 'send'],
    ['idle empty', { actionable: false }, 'enter', 'Send', true, 'send'],
    ['running empty', { running: true, actionable: false }, 'enter', 'Stop', false, undefined],
    ['running empty cannot cancel', { running: true, actionable: false, cancellationAvailable: false }, 'enter', 'Stop', true, undefined],
    ['running blocked', { running: true, blocked: true }, 'enter', 'Stop', false, undefined],
    ['running blocked cannot cancel', { running: true, blocked: true, cancellationAvailable: false }, 'enter', 'Stop', true, undefined],
    ['running ordinary defaults Queue', { running: true }, 'enter', 'Queue', false, 'send'],
    ['running accelerated Steer', { running: true }, 'accelerated', 'Steer', false, 'steer'],
    ['running cancellation pending still accepts draft', { running: true, cancellationAvailable: false }, 'enter', 'Queue', false, 'send'],
    ['running pending upload', { running: true, uploadsPending: true }, 'enter', 'Send', true, 'send'],
    ['idle pending upload', { uploadsPending: true }, 'enter', 'Send', true, 'send'],
    ['running awaiting ack', { running: true, acknowledging: true }, 'enter', 'Send', true, 'send'],
    ['idle awaiting ack', { acknowledging: true }, 'enter', 'Send', true, 'send'],
    ['idle blocked', { blocked: true }, 'enter', 'Send', true, 'send'],
    ['idle command', { draftKind: 'command' }, 'enter', 'Run command', false, undefined],
    ['running command', { running: true, draftKind: 'command' }, 'enter', 'Run command', false, undefined],
    ['accelerated command never steers', { running: true, draftKind: 'command' }, 'accelerated', 'Run command', false, undefined],
    ['unsupported slash', { running: true, draftKind: 'unsupported-command' }, 'enter', 'Review command', false, undefined],
    ['command pending upload', { draftKind: 'command', uploadsPending: true }, 'enter', 'Run command', true, undefined],
    ['command awaiting ack', { draftKind: 'command', acknowledging: true }, 'enter', 'Run command', true, undefined],
  ])('%s', (_, patch, gesture, label, disabled, delivery) => {
    const result = composerSubmissionPolicy({ ...ready, ...patch }, gesture);
    expect(result.label).toBe(label); expect(result.disabled).toBe(disabled);
    expect('delivery' in result ? result.delivery : undefined).toBe(delivery);
  });
});
