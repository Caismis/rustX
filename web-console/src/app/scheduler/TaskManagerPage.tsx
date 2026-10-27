/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-schedule/TaskManagerPage; see PROVENANCE.md. */
import { useEffect, useRef, useState } from 'react';
import { useVirtualizer } from '@tanstack/react-virtual';
import clsx from 'clsx';
import type { AppServerClient } from '../../client/app-server';
import { useLocale } from '../../locale/react';
import { Button } from '../../presentation/primitives/Button';
import { Input } from '../../presentation/primitives/Input';
import { IconClockOutline16, IconSearchOutline16, IconPlusOutline16, IconCloseOutline16 } from '../../presentation/primitives/icons';
import { useTaskCatalog, taskKey, taskRef, type TaskRef } from './model';
import { TaskDetail } from './TaskDetail';
import { useTaskCopy } from './copy';
import { frequency, displayInstant } from './timing';
import css from './TaskManager.module.css';
import extra from './scheduler.module.css';

type Filter = 'enabled' | 'disabled' | 'completed' | 'all';
export function TaskManagerPage({ client, newTask, openSession }: { client: AppServerClient; newTask: () => void; openSession: (ref: TaskRef) => void }) {
  const { model, records, connected, loading, issue } = useTaskCatalog(client), t = useTaskCopy(), locale = useLocale().active;
  useEffect(() => model.watchAll(), [model]);
  const [filter, setFilter] = useState<Filter>('enabled'), [query, setQuery] = useState('');
  const [selected, setSelected] = useState<{ ref: TaskRef; title: string }>();
  const [contentMatches, setContentMatches] = useState<{ query: string; keys: ReadonlySet<string> }>();
  const [searching, setSearching] = useState(false), [searchFailed, setSearchFailed] = useState(false);
  const [now, setNow] = useState(Date.now);
  useEffect(() => { const timer = window.setInterval(() => setNow(Date.now()), 30_000); return () => window.clearInterval(timer); }, []);
  useEffect(() => { document.title = t('title') + ' — rustX'; return () => { document.title = 'rustX'; }; }, [locale]);
  const search = query.trim().toLocaleLowerCase();
  const recordVersion = records.map(row => taskKey(taskRef(row)) + ':' + row.revision).join('|');
  useEffect(() => {
    let current = true;
    setSearchFailed(false);
    if (!search || loading || !connected) { setSearching(false); return; }
    setSearching(true);
    const timer = window.setTimeout(() => {
      void model.searchContent(search, () => current).then(keys => { if (current) setContentMatches({ query: search, keys }); })
        .catch(() => { if (current) setSearchFailed(true); }).finally(() => { if (current) setSearching(false); });
    }, 250);
    return () => { current = false; window.clearTimeout(timer); };
  }, [search, recordVersion, loading, connected, model]);
  const rows = records.filter(row => (filter === 'all' || row.state === filter)
    && (!search || contentMatches?.query === search && contentMatches.keys.has(taskKey(taskRef(row))) || [row.title, row.binding.session_id, row.target.type === 'workflow' ? row.target.workflow_id : ''].some(value => value.toLocaleLowerCase().includes(search))))
    .sort((a, b) => (a.next_due_utc ?? '9999').localeCompare(b.next_due_utc ?? '9999') || a.title.localeCompare(b.title));
  const scroll = useRef<HTMLDivElement>(null);
  const virtual = useVirtualizer({ count: rows.length, getScrollElement: () => scroll.current, estimateSize: () => 80, gap: 2, overscan: 5, initialRect: { width: 700, height: 800 } });
  const selectedKey = selected && taskKey(selected.ref);
  const status = (state: Filter) => t(state === 'enabled' ? 'status.active' : state === 'disabled' ? 'status.paused' : state === 'completed' ? 'status.completed' : 'statusFilter.all');
  return <section className={clsx(css.page, selected && css.hasDetails)} aria-label={t('title')} data-shell-route="schedules">
    <div className={css.listPane}><div ref={scroll} className={css.pageScroll}><div className={css.pageContent}>
      <header className={css.pageHeading}><h1>{t('title')}</h1><div className={css.creationActions}>
        <Button size="sm" className={css.headerAction} disabled={!connected || loading} onClick={() => void model.refresh()}>{t('action.refresh')}</Button>
        <Button variant="primary" size="sm" className={css.headerAction} icon={<IconPlusOutline16/>} onClick={newTask}>{t('new.action')}</Button>
      </div></header>
      <div className={css.filters}><div className={css.filterTabs} role="group" aria-label={t('statusFilter.label')}>
        {(['enabled', 'disabled', 'completed', 'all'] as const).map(value => <button key={value} type="button" className={clsx(css.filterTab, filter === value && css.filterTabActive)} aria-pressed={filter === value} onClick={() => setFilter(value)}>{status(value)}</button>)}
      </div></div>
      <div className={css.searchTools}><div className={css.searchField}><Input type="search" icon={<IconSearchOutline16/>} aria-label={t('search.label')} placeholder={t('search.placeholder')} value={query} onChange={event => setQuery(event.target.value)}/>
        {query && <Button size="sm" className={css.searchClear} aria-label={t('search.clear')} onClick={() => setQuery('')}><IconCloseOutline16/></Button>}
      </div></div>
      {!connected && <p className={css.notice} role="status">{t('connection.disconnected')}</p>}
      {issue && <div className={css.notice} role="alert"><p>{t('list.partial')}</p><Button size="sm" disabled={!connected || loading} onClick={() => void model.refresh()}>{t('list.retry')}</Button></div>}
      {loading && <p role="status" className={extra.loading}>{t(records.length ? 'list.refreshing' : 'list.loading')}</p>}
      {searching && <p role="status" className={extra.loading}>{t('search.content-loading')}</p>}
      {searchFailed && <p role="alert" className={extra.loading}>{t('search.content-error')}</p>}
      {!loading && !searching && rows.length === 0 && <div className={css.empty}><span className={css.emptyGlyph}><IconClockOutline16 size={32}/></span>
        <h2>{t(search ? 'list.noMatches' : filter === 'disabled' ? 'list.paused-empty' : filter === 'completed' ? 'list.ended-empty' : 'list.empty')}</h2>
        {!search && <Button className={css.emptyAction} onClick={newTask}>{t('empty.action')}</Button>}
      </div>}
      <ul className={css.listRows} aria-label={t('list.label')} aria-busy={loading} style={{ height: virtual.getTotalSize(), position: 'relative' }}>
        {virtual.getVirtualItems().map(item => { const row = rows[item.index], key = taskKey(taskRef(row)), next = row.state === 'enabled' && row.next_due_utc && displayInstant(row.next_due_utc, locale, now);
          return <li key={key} ref={virtual.measureElement} data-index={item.index} style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: 'translateY(' + item.start + 'px)' }}>
            <Button className={clsx(css.row, selectedKey === key && css.selectedRow, row.state !== 'enabled' && css.endedRow)} aria-expanded={selectedKey === key}
              onClick={() => setSelected({ ref: taskRef(row), title: row.title })}>
              <span className={css.rowGlyph}><IconClockOutline16/></span><span className={css.rowContent}>
                <span className={css.rowTitle}>{row.title}</span><span className={css.rowSummary}><span className={css.metadata}>{status(row.state)}</span><span className={css.metadata}>{frequency(row.spec, t, locale)}</span>
                  {row.target.type === 'workflow' && <span className={css.metadata}>{t('target.workflow')}</span>}{next && <span className={css.metadata}>{t('detail.nextRun')} · {next.absolute} ({next.relative})</span>}
                </span>
              </span>
            </Button>
          </li>;
        })}
      </ul>
    </div></div></div>
    {selected && <TaskDetail key={selectedKey} client={client} task={selected.ref} title={selected.title} close={() => setSelected(undefined)} openSession={openSession}/>}
  </section>;
}
