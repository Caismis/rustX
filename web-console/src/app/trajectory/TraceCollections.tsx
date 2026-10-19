import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { CheckpointPage, CheckpointPageItem, CollectionFrontier } from '../../../../protocol/app-server/v50';
import { validateCanonicalInspectionPage } from '../../../../protocol/app-server/trace-inspection';
import type { TraceFieldReader } from '../../client/trace';
import { useTranslation } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { SourceContent } from '../agent/SourceContent';
import css from './Trajectory.module.css';

/** Canonical result items render as ordinary detail content, without a source browser. */
export function TraceCollections({ collections, page, reader, owned, domain }: { collections: readonly CollectionFrontier[]; page: CheckpointPage; reader?: TraceFieldReader; owned: () => boolean; domain?: string }) {
  const tx = useTranslation(), frontier = collections.find(value => value.domain === domain) ?? collections[0];
  const cutKey = JSON.stringify(page.read_cut), cut = useMemo(() => page.read_cut, [cutKey]);
  const identity = JSON.stringify([frontier, cutKey]), latest = useRef(identity), latestOwned = useRef(owned); latest.current = identity; latestOwned.current = owned;
  const [state, setState] = useState<{ identity: string; reader: typeof reader; items: CheckpointPageItem[]; next?: number; pending?: boolean; error?: string }>();
  const loaded = state?.identity === identity && state.reader === reader ? state : undefined;
  const generation = useRef(0), active = owned();
  const content = useCallback((reference: Parameters<TraceFieldReader['content']>[1], offset: number, current: () => boolean) => reader!.content(cut, reference, offset, current), [reader, cut]);
  useEffect(() => () => { generation.current++; }, [identity, reader]);
  const load = async (offset = 0) => {
    if (!reader || !frontier || !latestOwned.current()) return;
    const token = ++generation.current, current = () => token === generation.current && latest.current === identity && latestOwned.current();
    setState({ identity, reader, items: offset ? loaded?.items ?? [] : [], pending: true });
    try {
      const next = await reader.page(cut, frontier.domain, offset, current);
      if (!current()) return;
      if (next.domain !== frontier.domain || next.cursor !== next.read_cut.revision || JSON.stringify(next.read_cut) !== cutKey || new TextEncoder().encode(JSON.stringify(next)).length > 786432) throw Error('Original canonical page changed.');
      validateCanonicalInspectionPage(next, offset, frontier.total, offset !== 0);
      setState({ identity, reader, items: [...(offset ? loaded?.items ?? [] : []), ...next.items], next: next.next_offset == null ? undefined : Number(next.next_offset) });
    } catch (error) { if (current()) setState({ identity, reader, items: loaded?.items ?? [], error: String(error).slice(0, 512) }); }
  };
  useEffect(() => { void load(); }, [identity, reader, active]);
  if (!frontier) return null;
  return <>
    {loaded?.items.map(item => <div key={item.offset}>
      {item.content.length ? <SourceContent explicit bindings={item.content.filter(binding=>binding.content_ref.representation!=='binary_bytes')} owned={owned} read={content}>{text => <pre className={css.payload}>{text}</pre>}</SourceContent>
        : item.value != null && <pre className={css.payload}>{JSON.stringify(item.value)}</pre>}
    </div>)}
    {loaded?.pending && <p role="status">{tx('trajectory:details.loading')}</p>}
    {loaded?.next !== undefined && <Button disabled={loaded.pending || !active} onClick={() => void load(loaded.next)}>{tx('common:activity.show-more')}</Button>}
    {loaded?.error && <p role="alert">{loaded.error}</p>}
  </>;
}
