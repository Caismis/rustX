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
export type ComposerAction = { disabled: boolean; title: string } & (
  | { kind: 'stop'; label: 'Stop' }
  | { kind: 'command'; label: 'Run command' | 'Review command' }
  | { kind: 'message'; label: 'Send' | 'Queue' | 'Steer'; delivery: 'send' | 'steer' }
);
export function composerSubmissionPolicy(facts: ComposerSubmissionFacts, gesture: SubmitGesture = 'enter'): ComposerAction {
  if (facts.running && (!facts.actionable || facts.blocked)) {
    return { kind: 'stop', label: 'Stop', title: 'Stop', disabled: !facts.cancellationAvailable };
  }
  const disabled = facts.blocked || facts.acknowledging || facts.uploadsPending || !facts.actionable;
  if (facts.draftKind !== 'message') {
    const label = facts.draftKind === 'command' ? 'Run command' : 'Review command';
    return { kind: 'command', label, title: label, disabled };
  }
  const delivery = facts.running && gesture === 'accelerated' ? 'steer' : 'send';
  const label = disabled || !facts.running ? 'Send' : delivery === 'steer' ? 'Steer' : 'Queue';
  const title = facts.acknowledging ? 'Awaiting acknowledgement…' : facts.uploadsPending ? 'Resolve draft uploads before sending'
    : label === 'Queue' ? 'Queue · Enter (Ctrl/Cmd+Enter to Steer)' : label;
  return { kind: 'message', label, title, delivery, disabled };
}
