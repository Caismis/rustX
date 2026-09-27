/** A UI continuation fence, never a cancellation token for server mutations. */
export class NavigationEpoch {
  private epoch = 0;
  invalidate() { this.epoch++; }
  capture() { const epoch = this.epoch; return () => this.epoch === epoch; }
}
