/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-deliverables/ChangedFiles; see PROVENANCE.md. */
import { useState, type ReactNode } from 'react';
import { useTranslation } from '../../locale/react';
import { FileTypeIcon } from '../primitives/FileTypeIcon';
import { HoverCard } from '../primitives/HoverCard';
import { IconChevronDownOutline14 } from '../primitives/icons';
import css from './ChangedFiles.module.css';

export interface FileChange {path:string;display:string;added:number;deleted:number;binary:boolean;oversized:boolean}
export interface Changes {files:FileChange[];total:number;added:number;deleted:number}
export function ChangedFiles({changes, openReview, preview}: {changes:Changes;openReview:(index:number)=>void;preview:(index:number)=>ReactNode}) {
  const tx = useTranslation(), [expanded,setExpanded] = useState(false);
  const single = changes.total === 1 ? changes.files[0] : undefined;
  const rows = expanded ? changes.files : changes.files.slice(0,4);
  const counts = (file: Pick<FileChange,'added'|'deleted'> & Partial<FileChange>) => file.binary ? tx('artifacts:changes.binary') : file.oversized ? tx('artifacts:changes.oversized') : <><span className={css.added}>+{file.added.toLocaleString('en-US')}</span><span className={css.deleted}>-{file.deleted.toLocaleString('en-US')}</span></>;
  const title = single ? tx('artifacts:changes.single-title',{name:single.display.split('/').at(-1) ?? single.display}) : tx('artifacts:changes.title',{count:changes.total});
  const header = <button type="button" className={css.header} aria-label={single ? tx('artifacts:changes.view-diff',{name:single.display}) : tx('artifacts:changes.open-review')} onClick={()=>openReview(0)}>
    <span className={css.tile}>{single ? <FileTypeIcon path={single.path} size={20}/> : <FileTypeIcon kind="code" size={20}/>}</span>
    <span className={css.titles}><span className={css.title}>{title}</span><span className={css.stat}><span className={css.statCounts}>{counts(single ?? changes)}</span><span className={css.previewHint}>{tx('artifacts:changes.open-review')}</span></span></span>
  </button>;
  const hover = (anchor:ReactNode,index:number) => <HoverCard anchor={anchor} content={preview(index)} openDelayMs={500} copyLabel={tx('artifacts:changes.copy')} copiedLabel={tx('artifacts:changes.copied')}/>;
  return <div className={css.card} data-changed-files data-single={!!single || undefined}>
    {single ? hover(header,0) : header}
    {!single && <ul className={css.list}>{rows.map((file,index)=><li key={index}>{hover(<button type="button" className={css.row} title={file.path} aria-label={tx('artifacts:changes.view-diff',{name:file.display})} onClick={()=>openReview(index)}><span className={css.path}>{file.display}</span><span className={css.counts}>{counts(file)}</span></button>,index)}</li>)}</ul>}
    {changes.files.length>4 && <button type="button" className={css.toggle} aria-expanded={expanded} onClick={()=>setExpanded(value=>!value)}>{tx(expanded?'artifacts:changes.collapse':'artifacts:changes.all',{count:changes.files.length})}<span style={expanded?{transform:'rotate(180deg)'}:undefined}><IconChevronDownOutline14/></span></button>}
  </div>;
}
