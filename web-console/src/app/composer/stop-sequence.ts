/* Copyright (c) 2026 DeepSeek. MIT. Adapted from Harness stop-sequence.ts; see PROVENANCE.md. */
export const STOP_SEQUENCE_MS = 500;
/** Exact client authority plus generation/attachment/Session/binding/Attempt identity. */
export interface StopScope { readonly authority: object; readonly identity: string }

/** Gesture recognition only; owns no cancellation state or retries. */
export class StopSequence {
  private revision = 0;
  private first?: { scope: StopScope; deadline: number };
  private timer?: ReturnType<typeof setTimeout>;
  reset = () => { this.revision++; this.first = undefined; clearTimeout(this.timer); this.timer = undefined; };
  /** Capture disarms synchronously. Only an unconsumed editor event accepts it. */
  prepare(): (scope: StopScope, cancel: () => void) => void {
    const first = this.first;
    this.reset(); // Release before foreground arbitration and native dispatch.
    const revision = this.revision;
    let accepted = false;
    return (scope, cancel) => {
      if (accepted || this.revision !== revision) return;
      accepted = true;
      if (first && performance.now() <= first.deadline && first.scope.authority === scope.authority && first.scope.identity === scope.identity) {
        cancel();
        return;
      }
      this.first = { scope, deadline: performance.now() + STOP_SEQUENCE_MS };
      this.timer = setTimeout(this.reset, STOP_SEQUENCE_MS + 1);
    };
  }
}
