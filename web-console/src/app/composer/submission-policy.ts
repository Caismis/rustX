import type { TranslationKey } from '../../locale/translation';
/* Copyright (c) 2026 DeepSeek. MIT. Interaction policy adapted from Harness; see PROVENANCE.md. */
/** Presentation only: resolve a gesture, never admit, persist, retry or settle it.
 * Plain Enter and the button queue by default; Ctrl/Cmd+Enter steers a new draft.
 * rustX exposes no busy-Enter preference or continuable-child composer scope. */
export interface ComposerSubmissionFacts {
  running: boolean;
  actionable: boolean;
  draftKind: 'message' | 'command' | 'unsupported-command';
  blocked: boolean;
  acknowledging: boolean;
  uploadsPending: boolean;
  cancellationAvailable: boolean;
}
export type SubmitGesture = 'enter' | 'accelerated';
export type ComposerAction = { disabled: boolean; title: TranslationKey } & (
  | { kind: 'stop'; label: 'agent:submission.stop' }
  | { kind: 'command'; label: 'agent:submission.run-command' | 'agent:submission.review-command' }
  | { kind: 'message'; label: 'agent:submission.send' | 'agent:submission.queue' | 'agent:submission.steer'; delivery: 'send' | 'steer' }
);
export function composerSubmissionPolicy(facts: ComposerSubmissionFacts, gesture: SubmitGesture = 'enter'): ComposerAction {
  if (facts.running && (!facts.actionable || facts.blocked)) {
    return { kind: 'stop', label: 'agent:submission.stop', title: 'agent:submission.stop', disabled: !facts.cancellationAvailable };
  }
  const disabled = facts.blocked || facts.acknowledging || facts.uploadsPending || !facts.actionable;
  if (facts.draftKind !== 'message') {
    const label = facts.draftKind === 'command' ? 'agent:submission.run-command' : 'agent:submission.review-command';
    return { kind: 'command', label, title: label, disabled };
  }
  const delivery = facts.running && gesture === 'accelerated' ? 'steer' : 'send';
  const label = disabled || !facts.running ? 'agent:submission.send' : delivery === 'steer' ? 'agent:submission.steer' : 'agent:submission.queue';
  const title = facts.acknowledging ? 'agent:submission.awaiting-acknowledgement' : facts.uploadsPending ? 'agent:submission.resolve-draft-uploads-before-sending'
    : label === 'agent:submission.queue' ? 'agent:submission.queue-enter-ctrl-cmd-enter-to-steer' : label;
  return { kind: 'message', label, title, delivery, disabled };
}
