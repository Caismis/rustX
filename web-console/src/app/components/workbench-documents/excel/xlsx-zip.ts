/** Admit the complete XLSX directory before allocating any expanded member. */
import { inflateSync, type Unzipped } from 'fflate'
import { buf as crc32 } from 'crc-32'
import { FILE_PREVIEW_LIMITS } from '../../../../../shared/file-preview.ts'

export const XLSX_ZIP_LIMITS = Object.freeze({
  source: FILE_PREVIEW_LIMITS.spreadsheetBytes, entries: 10000, entry: 32 * 1024 * 1024,
  expanded: 64 * 1024 * 1024, path: 240, ratio: 200,
})
type Entry = { name: string; local: number; start: number; end: number; next: number; size: number; crc: number; method: number }
const reject = (): never => { throw new Error('archive_rejected') }

/** Fixed output buffers bound actual inflation independently of ZIP size declarations. */
export function readXlsxZip(bytes: Uint8Array<ArrayBuffer>): Unzipped {
  if (bytes.length < 22 || bytes.length > XLSX_ZIP_LIMITS.source) return reject()
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const u16 = (at: number) => view.getUint16(at, true)
  const u32 = (at: number) => view.getUint32(at, true)
  let end = bytes.length - 22
  const earliest = Math.max(0, end - 65535)
  while (end >= earliest && (u32(end) !== 0x06054b50 || end + 22 + u16(end + 20) !== bytes.length)) end--
  if (end < earliest) return reject()
  const count = u16(end + 10), directory = u32(end + 16)
  if (u16(end + 4) || u16(end + 6) || count !== u16(end + 8) || !count
    || count > XLSX_ZIP_LIMITS.entries || directory + u32(end + 12) !== end) return reject()
  const entries: Entry[] = [], names = new Set<string>(), files = new Set<string>(), parents = new Set<string>()
  const decoder = new TextDecoder('utf-8', { fatal: true })
  let at = directory, expanded = 0
  for (let n = 0; n < count; n++) {
    if (at + 46 > end || u32(at) !== 0x02014b50) return reject()
    const flags = u16(at + 8), method = u16(at + 10), crc = u32(at + 16)
    const compressed = u32(at + 20), size = u32(at + 24), length = u16(at + 28)
    const extra = u16(at + 30), comment = u16(at + 32), local = u32(at + 42)
    if (at + 46 + length + extra + comment > end || u16(at + 34) || flags & ~0x080e
      || (method !== 0 && method !== 8) || !length || length > XLSX_ZIP_LIMITS.path
      || size > XLSX_ZIP_LIMITS.entry || size > Math.max(1, compressed) * XLSX_ZIP_LIMITS.ratio
      || (method === 0 && size !== compressed)) return reject()
    const name = decoder.decode(bytes.subarray(at + 46, at + 46 + length))
    const key = name.replace(/[A-Z]/g, character => character.toLowerCase()).replace(/\/$/, '')
    if (!key || /[\\:\x00-\x1f\x7f%]/.test(name) || key.split('/').some(p => !p || p === '.' || p === '..')
      || names.has(key) || files.has(key) || (!name.endsWith('/') && parents.has(key))
      || (name.endsWith('/') && size !== 0)) return reject()
    names.add(key)
    if (!name.endsWith('/')) files.add(key)
    for (let slash = key.indexOf('/'); slash >= 0; slash = key.indexOf('/', slash + 1)) {
      const parent = key.slice(0, slash)
      if (files.has(parent)) return reject()
      parents.add(parent)
    }
    expanded += size
    if (expanded > XLSX_ZIP_LIMITS.expanded || local + 30 > directory || u32(local) !== 0x04034b50
      || u16(local + 6) !== flags || u16(local + 8) !== method) return reject()
    const localLength = u16(local + 26), localExtra = u16(local + 28)
    const start = local + 30 + localLength + localExtra, finish = start + compressed
    if (localLength !== length || finish > directory || start > directory
      || !bytes.subarray(local + 30, local + 30 + length).every((byte, i) => byte === bytes[at + 46 + i])) return reject()
    if (flags & 8) {
      // Streaming writers may defer the three values to a signed or unsigned descriptor.
      if ((u32(local + 14) !== 0 && u32(local + 14) !== crc)
        || (u32(local + 18) !== 0 && u32(local + 18) !== compressed)
        || (u32(local + 22) !== 0 && u32(local + 22) !== size)) return reject()
    } else if (u32(local + 14) !== crc || u32(local + 18) !== compressed || u32(local + 22) !== size) return reject()
    for (const [offset, extraLength] of [[at + 46 + length, extra], [local + 30 + localLength, localExtra]]) {
      for (let p = offset; p < offset + extraLength;) {
        if (p + 4 > offset + extraLength) return reject()
        const id = u16(p), size = u16(p + 2)
        // Reject ZIP64 and Unicode path overrides instead of reconciling alternate identities.
        if (id === 1 || id === 0x7075 || p + 4 + size > offset + extraLength) return reject()
        p += 4 + size
      }
    }
    let next = finish
    if (flags & 8) {
      const matches = (offset: number) => offset + 12 <= directory
        && u32(offset) === crc && u32(offset + 4) === compressed && u32(offset + 8) === size
      if (matches(next)) next += 12
      else if (next + 16 <= directory && u32(next) === 0x08074b50 && matches(next + 4)) next += 16
      else return reject()
    }
    entries.push({ name, local, start, end: finish, next, size, crc, method })
    at += 46 + length + extra + comment
  }
  if (at !== end) return reject()
  let boundary = 0
  for (const entry of [...entries].sort((a, b) => a.local - b.local)) {
    if (entry.local !== boundary) return reject()
    boundary = entry.next
  }
  if (boundary !== directory) return reject()
  const result: Unzipped = Object.create(null)
  for (const entry of entries) {
    const payload = bytes.subarray(entry.start, entry.end)
    // fflate's supplied output never grows. The extra byte distinguishes overflow
    // from an exact result; allocating only size would silently accept truncation.
    const data = entry.method === 0 ? payload : inflateSync(payload, { out: new Uint8Array(entry.size + 1) })
    if (data.length !== entry.size || (crc32(data) >>> 0) !== entry.crc) return reject()
    result[entry.name] = data
  }
  return result
}
