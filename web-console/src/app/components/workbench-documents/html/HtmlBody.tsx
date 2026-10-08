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

type Props = { data: Uint8Array<ArrayBuffer>; path: string; call: WorkbenchCall; signal: AbortSignal };
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
function HtmlFrame({ data, path, call, signal }: Props) {
  const tx = useTranslation();
  const [frame, setFrame] = useState<{data: Uint8Array; url?: string; failed?: boolean}>();
  useEffect(() => {
    const lifetime = new AbortController();
    const current = AbortSignal.any([signal, lifetime.signal]);
    let url: string | undefined;
    void packHtml(data, async (reference, reading) => {
      const resolved = relativeDocumentPath(path, reference);
      if (!resolved) throw new Error('HTML dependency must stay inside the workspace');
      const result = await call({ kind: 'bytes', path: resolved }, reading);
      reading.throwIfAborted();
      if (result.base64 === undefined) throw new Error('HTML dependency unavailable');
      return { data: Uint8Array.from(atob(result.base64), character => character.charCodeAt(0)) };
    }, current).then(bundle => {
      current.throwIfAborted();
      url = URL.createObjectURL(new Blob([createHtmlDocument(bundle)], {type:'text/html'}));
      setFrame({data, url});
    }).catch(() => { if (!current.aborted) setFrame({data, failed:true}); });
    return () => { lifetime.abort(); if (url) URL.revokeObjectURL(url); };
  }, [data, path, call, signal]);
  if (frame?.data !== data) return <p role="status">{tx('artifacts:workbench.loading')}</p>;
  if (frame.failed) return <p role="alert">{tx('artifacts:workbench.html-failed')}</p>;
  return <iframe key={frame.url} className={css.frame} src={frame.url} sandbox="allow-scripts" title="HTML" data-html-preview/>;
}
