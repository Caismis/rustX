import { useEffect, useRef, useState } from 'react';
import type { CheckpointPage } from '../../../../protocol/app-server/v50';
import type { TraceFieldReader } from '../../client/trace';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { TraceValue } from './TraceFields';

export const REQUEST_OPTION_FIELDS = ['model','protocol','reasoning_enabled','profile','max_output_tokens','context_window_tokens','options','contributions'] as const;
export type RequestField = typeof REQUEST_OPTION_FIELDS[number] | 'effective_system_prompt' | 'previous_system_prompt' | 'tools';
const FIELD_LABEL = {model:'details.model',protocol:'field.protocol',reasoning_enabled:'field.reasoning',profile:'field.profile',max_output_tokens:'field.max-output',context_window_tokens:'field.context-window',options:'tab.options',contributions:'field.contributions',effective_system_prompt:'field.current-prompt',previous_system_prompt:'field.previous-prompt',tools:'tab.tools'} as const;
function selectedPage(page: CheckpointPage, original: CheckpointPage, domain: string, offset: number) {
  if (!Array.isArray(page.items) || page.items.length > 32 || page.domain !== domain || page.cursor !== page.read_cut.revision || JSON.stringify(page.read_cut) !== JSON.stringify(original.read_cut)
    || typeof page.total !== 'string' || !/^(0|[1-9][0-9]{0,19})$/.test(page.total) || BigInt(page.total) > 18446744073709551615n) throw Error('Original request field page changed.');
  const total = BigInt(page.total), end = offset + page.items.length, next = Number.isSafeInteger(end) && BigInt(end) < total ? String(end) : null;
  if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isSafeInteger(end) || BigInt(end) > total || end === offset && BigInt(offset) < total || (page.next_offset ?? null) !== next
    || page.items.some((item, index) => item.offset !== String(offset + index))) throw Error('Original request field page did not advance.');
  return page;
}

/** Request tabs load the selected field directly, retaining its exact native cut. */
export function TraceRequestFields({ page, reader, fields, previousDomain, onEarlier }: { page: CheckpointPage; reader?: TraceFieldReader; fields: readonly RequestField[]; previousDomain?: string; onEarlier?: () => void }) {
  const tx = useTranslation(), [selection, setSelection] = useState<RequestField>(fields[0]);
  const field = fields.includes(selection) ? selection : fields[0], domain = field === 'previous_system_prompt' ? previousDomain : `${page.domain}.request.${field}`;
  const identity = JSON.stringify([page.domain, page.read_cut, field, domain]), latest = useRef(identity); latest.current = identity;
  const generation = useRef(0), [state, setState] = useState<{ identity: string; reader: typeof reader; head?: CheckpointPage; error?: string; pending?: boolean }>();
  const loaded = state?.identity === identity && state.reader === reader ? state : undefined;
  const load = async (offset = 0) => {
    if (!reader || !domain || !/^trace\.records\.(0|[1-9][0-9]*)\.detail$/.test(page.domain) || fields.length > 8) return;
    const token = ++generation.current, current = () => token === generation.current && latest.current === identity;
    setState({ identity, reader, head: offset ? loaded?.head : undefined, pending: true });
    try {
      const head = selectedPage(await reader.page(page.read_cut, domain, offset, current), page, domain, offset);
      if (current()) setState({ identity, reader, head: offset && loaded?.head ? { ...head, items: [...loaded.head.items, ...head.items] } : head });
    } catch (error) { if (current()) setState({ identity, reader, error: String(error).slice(0, 512) }); }
  };
  useEffect(() => { void load(); return () => { generation.current++; }; }, [identity, reader]);
  return <section aria-label={tx('trajectory:options.json')}>
    {fields.length > 1 && <select aria-label={tx('trajectory:options.json')} value={field} onChange={event => setSelection(event.target.value as RequestField)}>{fields.map(name => <option key={name} value={name}>{tx(`trajectory:${FIELD_LABEL[name]}`)}</option>)}</select>}
    {!domain && field === 'previous_system_prompt' && <p role="status">{tx('trajectory:field.previous-not-loaded')}{onEarlier && <Button onClick={onEarlier}>{tx('trajectory:history.load-earlier')}</Button>}</p>}
    {domain && loaded?.head && (loaded.head.total === '0' ? <TraceValue field={{ domain, item_offset: 0, path: [] }} cut={page.read_cut} reader={reader} owned={() => latest.current === identity}/>
      : loaded.head.items.map(row => <div key={row.offset}>{row.key?.preview && <strong>{row.key.preview}</strong>}<TraceValue field={{ domain, item_offset: Number(row.offset), path: [] }} cut={page.read_cut} reader={reader} owned={() => latest.current === identity}/></div>))}
    {loaded?.pending && <p role="status">{tx('trajectory:details.loading')}</p>}
    {loaded?.head?.next_offset != null && <Button disabled={loaded.pending} onClick={() => void load(Number(loaded.head!.next_offset))}>{tx('common:activity.show-more')}</Button>}
    {loaded?.error && <p role="alert">{loaded.error}</p>}
  </section>;
}
