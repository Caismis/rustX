import { parseObservedValue } from './domain-observation.ts';
import type { BoundedSessionSettings, SessionPersistentState, SessionSettingsRef, SessionSettingsChunk } from './v57.ts';
/** Materialize one settings cut with a finite client byte cache. */
export async function materializeSettings(view: BoundedSessionSettings, read: (ref: SessionSettingsRef, offset: number) => Promise<SessionSettingsChunk>, current: () => boolean): Promise<SessionPersistentState> {
  if (!current()) throw new Error('Settings view was replaced.');
  if (view.inline) return view.inline;
  const ref = view.reference;
  if (!ref || ref.revision !== view.revision || !Number.isSafeInteger(ref.byte_length) || ref.byte_length < 0 || ref.byte_length > 16 * 1024 * 1024) throw new Error('Settings detail exceeds the loaded byte cache.');
  let offset = 0; const bytes = new Uint8Array(ref.byte_length);
  while (offset < ref.byte_length) {
    const chunk = await read(ref, offset);
    if (!current()) throw new Error('Settings view was replaced.');
    if (JSON.stringify(chunk.reference) !== JSON.stringify(ref) || chunk.offset !== offset || !Number.isSafeInteger(chunk.end_offset) || chunk.end_offset <= offset || chunk.end_offset > Math.min(ref.byte_length, offset + 65_536)) throw new Error('Settings range identity or progress mismatch.');
    if(chunk.data_base64.length>87_384 || chunk.data_base64.length%4!==0 || chunk.data_base64.length/4*3-(chunk.data_base64.endsWith('==')?2:chunk.data_base64.endsWith('=')?1:0)>65_536)throw new Error('Encoded native range exceeds the decoded byte bound.');
    const decoded = Uint8Array.from(atob(chunk.data_base64), value => value.charCodeAt(0));
    if (decoded.length !== chunk.end_offset - offset || (chunk.next_offset ?? null) !== (chunk.end_offset < ref.byte_length ? chunk.end_offset : null)) throw new Error('Settings range evidence mismatch.');
    bytes.set(decoded, offset); offset = chunk.end_offset;
  }
  const settings = parseObservedValue(new TextDecoder('utf-8', {fatal:true}).decode(bytes)) as SessionPersistentState;
  if (settings.cwd !== view.cwd) throw new Error('Settings cwd identity mismatch.');
  return settings;
}
