/** Observation readiness and terminal physical cleanup, shared by the Node
 * Host and browser adapters without importing either runtime's state owner. */
export interface PreviewObservation { version: string; close(): Promise<void>; done: Promise<void> }
