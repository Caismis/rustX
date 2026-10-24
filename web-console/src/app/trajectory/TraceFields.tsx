import { useEffect, useRef, useState, useCallback, useMemo } from 'react';
import type { CheckpointReadCut, TraceFieldSource } from '../../../../protocol/app-server/v57';
import { traceFieldKey } from '../../../../protocol/app-server/trace-identity';
import { readTraceFieldWindow, readTraceSelectedWindow, type TraceValueSelector, type TraceValueField } from '../../../../protocol/app-server/trace-field';
import type { TraceFieldReader } from '../../client/trace';
import { SourceContent } from '../agent/SourceContent';
import { useTranslation } from '../../locale/react';
import css from './Trajectory.module.css';

/** Opening a detail owns its reads; byte windows never become user controls. */
export function TraceFields({ fields, cut, reader, owned }: { fields: readonly TraceFieldSource[]; cut?: CheckpointReadCut; reader?: TraceFieldReader; owned: () => boolean }) {
  return <>{fields.map(field => <TraceField key={traceFieldKey(field)} field={field} cut={cut} reader={reader} owned={owned}/>)}</>;
}
function TraceField({ field, cut, reader, owned }: { field: TraceFieldSource; cut?: CheckpointReadCut; reader?: TraceFieldReader; owned: () => boolean }) {
  const key = JSON.stringify(cut), stableCut = useMemo(() => cut, [key]);
  const read = useCallback((reference: Parameters<TraceFieldReader['content']>[1], offset: number, current: () => boolean) => reader!.content(stableCut!, reference, offset, current), [reader, stableCut]);
  return field.type === 'content' ? <SourceContent explicit bindings={[field.binding]} owned={owned} read={reader && stableCut ? read : undefined}>{text => <pre className={css.payload}>{text}</pre>}</SourceContent>
    : <TraceValue field={field} cut={cut} reader={reader} owned={owned}/>;
}
export function TraceValue({ field, cut, reader, owned }: { field: TraceValueField | TraceValueSelector; cut?: CheckpointReadCut; reader?: TraceFieldReader; owned: () => boolean }) {
  const tx = useTranslation(), latestOwned = useRef(owned); latestOwned.current = owned;
  const identity = JSON.stringify([field, cut]);
  const latest = useRef(identity); latest.current = identity;
  const [state, setState] = useState<{ identity: string; reader: typeof reader; text?: string; error?: string }>();
  const loaded = state?.identity === identity && state.reader === reader ? state : undefined;
  const active = owned();
  useEffect(() => {
    if (!reader || !cut || !active) return;
    let mounted = true;
    const current = () => mounted && latest.current === identity && latestOwned.current();
    void (async () => {
      const decoder = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }), parts: string[] = [];
      let offset: number | undefined = 0, representation: string | undefined, end: number | undefined;
      do {
        const read = (at: number) => reader.value(cut, field, at, current);
        const chunk: Awaited<ReturnType<typeof readTraceSelectedWindow>> = 'encoded_bytes' in field ? await readTraceFieldWindow(field, cut, offset, read, current)
          : await readTraceSelectedWindow(field, cut, offset, read, current, end);
        end = chunk.readable_end;
        if (representation !== undefined && representation !== chunk.representation) throw Error('Trace field representation changed.');
        representation = chunk.representation;
        parts.push(decoder.decode(chunk.bytes, { stream: chunk.next !== undefined }));
        offset = chunk.next;
      } while (offset !== undefined && current());
      if (current()) setState({ identity, reader, text: parts.join('') });
    })().catch(error => { if (current()) setState({ identity, reader, error: String(error).slice(0, 512) }); });
    return () => { mounted = false; };
  }, [identity, reader, active]);
  return loaded?.error ? <p role="alert">{loaded.error}</p> : loaded?.text !== undefined ? <pre className={css.payload}>{loaded.text}</pre> : <p role="status">{tx(!reader || !cut ? 'trajectory:source.cut-unavailable' : 'trajectory:details.loading')}</p>;
}
