/* Copyright (c) 2026 DeepSeek. MIT. HtmlBody port; see PROVENANCE.md. */
import { useEffect, useMemo, useState } from 'react';
import { createHtmlDocument } from './bootstrap';
import { createBasicHtmlDocument } from '../basic-document';
import { packHtml } from './pack';
import { relativeDocumentPath } from '../resources';
import { useInteractiveHtml } from './preferences';
import { useTranslation } from '../../../../locale/react';
import type { WorkbenchCall } from '../../WorkbenchFiles';
import css from './HtmlBody.module.css';
import { DocumentDependencies } from '../dependencies';
import { documentFailure } from '../../documents/errors';

type Props = { data: Uint8Array<ArrayBuffer>; path: string; call: WorkbenchCall; signal: AbortSignal; dependencies?: DocumentDependencies };
export function HtmlBody(props: Props) {
  const [interactive] = useInteractiveHtml();
  return interactive ? <HtmlFrame {...props}/> : <BasicHtmlFrame data={props.data}/>;
}
function BasicHtmlFrame({ data }: Pick<Props, 'data'>) {
  const tx = useTranslation();
  const html = useMemo(() => { try { return createBasicHtmlDocument(data); } catch { return undefined; } }, [data]);
  if (html === undefined) return <p role="alert">{tx('artifacts:workbench.html-failed')}</p>;
  return <iframe className={css.frame} srcDoc={html} sandbox="" title="HTML" data-html-preview/>;
}
function HtmlFrame({ data, path, call, signal, dependencies: retained }: Props) {
  const tx = useTranslation();
  const [frame, setFrame] = useState<{data: Uint8Array; url?: string; error?: unknown}>();
  const [revision, refresh] = useState(0);
  const dependencies = useMemo(() => retained ?? new DocumentDependencies(call, () => refresh(value => value + 1), data.byteLength), [retained, call, path, data, revision]);
  useEffect(() => () => { if (!retained) dependencies.dispose(); }, [dependencies, retained]);
  useEffect(() => {
    const lifetime = new AbortController();
    const current = AbortSignal.any([signal, lifetime.signal]);
    let url: string | undefined;
    void packHtml(data, async (reference, reading) => {
      const resolved = relativeDocumentPath(path, reference);
      if (!resolved) throw new Error('HTML dependency must stay inside the workspace');
      const result = await dependencies.read(resolved, reading);
      reading.throwIfAborted();
      return { data: result.data };
    }, current).then(bundle => {
      current.throwIfAborted();
      url = URL.createObjectURL(new Blob([createHtmlDocument(bundle)], {type:'text/html'}));
      setFrame({data, url});
    }).catch(error => { if (!current.aborted) setFrame({data, error}); });
    return () => { lifetime.abort(); if (url) URL.revokeObjectURL(url); };
  }, [data, path, dependencies, signal]);
  if (frame?.data !== data) return <p role="status">{tx('artifacts:workbench.loading')}</p>;
  if (frame.error !== undefined) return <p role="alert">{tx(`artifacts:document.${documentFailure(frame.error)}`)}</p>;
  return <iframe key={frame.url} className={css.frame} src={frame.url} sandbox="allow-scripts" title="HTML" data-html-preview/>;
}
