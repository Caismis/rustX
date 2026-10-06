/** OOXML admission only. No archive path is ever extracted to the filesystem. */
import { crc32, inflateRawSync } from 'node:zlib';

export const OOXML_LIMITS = Object.freeze({
  source: 512 * 1024, entries: 256, entry: 4 * 1024 * 1024,
  expanded: 16 * 1024 * 1024, path: 240, ratio: 200,
});
type Entry = { name: string; start: number; end: number; size: number; crc: number; method: number };
const rejected = (): never => { throw new Error('archive_rejected'); };

/** Validate the complete directory and every local header before inflating anything.
 * ZIP64, encryption, split archives, ambiguous names and nested packages fail closed.
 * Inflation has a second hard output bound independent of attacker-declared sizes. */
export function admitOoxml(source: Uint8Array): Map<string, Buffer> {
  if (source.byteLength > OOXML_LIMITS.source) throw new Error('too_large');
  const bytes = Buffer.from(source.buffer, source.byteOffset, source.byteLength);
  if (bytes.length < 22) return rejected();
  let end = bytes.length - 22;
  while (end >= Math.max(0, bytes.length - 65557) && bytes.readUInt32LE(end) !== 0x06054b50) end--;
  if (end < 0 || bytes.readUInt32LE(end) !== 0x06054b50 || end + 22 + bytes.readUInt16LE(end + 20) !== bytes.length) return rejected();
  const count = bytes.readUInt16LE(end + 10), directory = bytes.readUInt32LE(end + 16);
  if (bytes.readUInt16LE(end + 4) || bytes.readUInt16LE(end + 6) || count !== bytes.readUInt16LE(end + 8)
    || !count || count > OOXML_LIMITS.entries || directory + bytes.readUInt32LE(end + 12) !== end) return rejected();
  const entries: Entry[] = [], names = new Set<string>();
  let at = directory, expanded = 0;
  const decoder = new TextDecoder('utf-8', { fatal: true });
  for (let n = 0; n < count; n++) {
    if (at + 46 > end || bytes.readUInt32LE(at) !== 0x02014b50) return rejected();
    const flags = bytes.readUInt16LE(at + 8), method = bytes.readUInt16LE(at + 10);
    const crc = bytes.readUInt32LE(at + 16), compressed = bytes.readUInt32LE(at + 20), size = bytes.readUInt32LE(at + 24);
    const length = bytes.readUInt16LE(at + 28), extra = bytes.readUInt16LE(at + 30), comment = bytes.readUInt16LE(at + 32);
    const local = bytes.readUInt32LE(at + 42);
    if (at + 46 + length + extra + comment > end || bytes.readUInt16LE(at + 34)
      || flags & ~0x0806 || (method !== 0 && method !== 8) || !length || length > OOXML_LIMITS.path
      || size > OOXML_LIMITS.entry || compressed > bytes.length || size > Math.max(1, compressed) * OOXML_LIMITS.ratio) return rejected();
    let name: string;
    try { name = decoder.decode(bytes.subarray(at + 46, at + 46 + length)); } catch { return rejected(); }
    if (name.startsWith('/') || /[\\:\x00-\x1f\x7f%]/.test(name) || name.split('/').some(p => p === '.' || p === '..' || !p)
      || /\.(?:zip|docx|pptx|xlsx|xlsm|exe|dll|bin)$/i.test(name)) return rejected();
    const key = name.toLowerCase();
    if (names.has(key) || [...names].some(p => p.startsWith(`${key}/`) || key.startsWith(`${p}/`))) return rejected();
    names.add(key);
    expanded += size;
    if (expanded > OOXML_LIMITS.expanded || local + 30 > directory || bytes.readUInt32LE(local) !== 0x04034b50
      || bytes.readUInt16LE(local + 6) !== flags || bytes.readUInt16LE(local + 8) !== method) return rejected();
    const localLength = bytes.readUInt16LE(local + 26), localExtra = bytes.readUInt16LE(local + 28);
    const start = local + 30 + localLength + localExtra, finish = start + compressed;
    if (localLength !== length || finish > directory || start > directory
      || !bytes.subarray(local + 30, local + 30 + localLength).equals(bytes.subarray(at + 46, at + 46 + length))) return rejected();
    if (!(flags & 8) && (bytes.readUInt32LE(local + 14) !== crc || bytes.readUInt32LE(local + 18) !== compressed || bytes.readUInt32LE(local + 22) !== size)) return rejected();
    // No extra-field extension is needed by this finite reader. Reject ZIP64 and
    // Unicode name overrides rather than reconciling competing identities.
    for (const [offset, size] of [[at + 46 + length, extra], [local + 30 + localLength, localExtra]]) {
      for (let p = offset; p < offset + size;) {
        if (p + 4 > offset + size) return rejected();
        const id = bytes.readUInt16LE(p), len = bytes.readUInt16LE(p + 2);
        if (id === 1 || id === 0x7075 || p + 4 + len > offset + size) return rejected();
        p += 4 + len;
      }
    }
    // Include local headers in overlap checks, not just compressed payloads.
    if (entries.some(e => local < e.end && finish > e.start)) return rejected();
    entries.push({ name, start: local, end: finish, size, crc, method });
    at += 46 + length + extra + comment;
  }
  if (at !== end) return rejected();
  let boundary = 0;
  for (const entry of [...entries].sort((a, b) => a.start - b.start)) {
    if (entry.start !== boundary) return rejected();
    boundary = entry.end;
  }
  if (boundary !== directory) return rejected();
  const files = new Map<string, Buffer>();
  for (const entry of entries) {
    const start = entry.start + 30 + bytes.readUInt16LE(entry.start + 26) + bytes.readUInt16LE(entry.start + 28);
    let data: Buffer;
    try {
      const payload = bytes.subarray(start, entry.end);
      data = entry.method === 0 ? Buffer.from(payload) : inflateRawSync(payload, { maxOutputLength: Math.max(1, entry.size) });
    } catch { return rejected(); }
    if (data.length !== entry.size || crc32(data) !== entry.crc || (data.length >= 4 && data.readUInt32LE(0) === 0x04034b50)) return rejected();
    files.set(entry.name, data);
  }
  if (!files.has('[Content_Types].xml') || !files.has('_rels/.rels')) return rejected();
  return files;
}
