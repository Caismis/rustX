/** Host-owned, content-addressed successful conversions. Source authority is
 * always checked by the operation before these bytes may reach a caller. */
export class OfficePreviewCache {
  private readonly entries = new Map<string, Buffer>();
  private bytes = 0;
  private readonly maxEntries: number;
  private readonly maxBytes: number;
  constructor(maxEntries = 8, maxBytes = 128 * 1024 * 1024) {
    if (!Number.isSafeInteger(maxEntries) || maxEntries < 1 || !Number.isSafeInteger(maxBytes) || maxBytes < 1) throw new Error('Invalid Office preview cache budget');
    this.maxEntries = maxEntries; this.maxBytes = maxBytes;
  }
  get(key: string): Buffer | undefined {
    const value = this.entries.get(key);
    if (value) { this.entries.delete(key); this.entries.set(key, value); }
    return value;
  }
  retain(key: string, value: Buffer) {
    if (value.byteLength > this.maxBytes) return;
    const previous = this.entries.get(key);
    if (previous) { this.entries.delete(key); this.bytes -= previous.byteLength; }
    while (this.entries.size >= this.maxEntries || this.bytes + value.byteLength > this.maxBytes) {
      const oldest = this.entries.keys().next().value!;
      this.bytes -= this.entries.get(oldest)!.byteLength; this.entries.delete(oldest);
    }
    this.entries.set(key, value); this.bytes += value.byteLength;
  }
  clear() { this.entries.clear(); this.bytes = 0; }
}
