/** A single authorized copy. The consumer acknowledges each bounded page before
 * more pages enter the decoder. Completion includes final source validation.
 * An owner may replay its retained validated raw copy through the same source. */
export interface GeometrySourceConsumer {
  start(metadata: { bytes: number; version: string }): void | Promise<void>;
  chunk(data: Uint8Array<ArrayBuffer>, offset: number): void | Promise<void>;
}
export interface GeometrySource {
  consume(consumer: GeometrySourceConsumer, signal: AbortSignal): Promise<{ version: string }>;
}
