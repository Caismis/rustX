/* Copyright (c) 2026 DeepSeek. MIT. Adapted from ui-deliverables/ChangedFiles; see PROVENANCE.md. */
import { useId, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from '../../locale/react';
import { FileTypeIcon } from '../primitives/FileTypeIcon';
import { HoverCard } from '../primitives/HoverCard';
import { IconChevronDownOutline14 } from '../primitives/icons';

export interface FileChange {path:string;display:string;added:number;deleted:number;binary:boolean;oversized:boolean}
export interface Changes {files:FileChange[];total:number;added:number;deleted:number}
export function ChangedFiles({changes, openReview, preview}: {changes:Changes;openReview:(index:number)=>void;preview:(index:number)=>ReactNode}) {
 const tx = useTranslation(), [expanded,setExpanded] = useState(false);
 const cardRef = useRef<HTMLDivElement>(null);
 const pathId = useId();
 const single = changes.total === 1 ? changes.files[0] : undefined;
 const rows = expanded ? changes.files : changes.files.slice(0,4);
 const counts = (file: Pick<FileChange,'added'|'deleted'> & Partial<FileChange>) => file.binary ? tx('artifacts:changes.binary') : file.oversized ? tx('artifacts:changes.oversized') : <><span className="[color:var(--dsw-alias-state-success-primary)]">+{file.added.toLocaleString('en-US')}</span><span className="[color:var(--dsw-alias-state-error-primary)]">-{file.deleted.toLocaleString('en-US')}</span></>;
 const title = single ? tx('artifacts:changes.single-title',{name:single.display.split('/').at(-1) ?? single.display}) : tx('artifacts:changes.title',{count:changes.total});
 const header = <button type="button" className="group flex items-center gap-[10px] box-border w-full min-w-0 h-[60px] m-0 px-[10px] py-[8px] border-0 bg-[var(--changes-fill)] [color:inherit] text-left cursor-pointer transition-colors duration-[120ms] hover:not-disabled:bg-[var(--changes-hover)] focus-visible:bg-[var(--changes-hover)] focus-visible:outline-none focus-visible:[box-shadow:inset_0_0_0_2px_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] disabled:cursor-progress" aria-label={single ? tx('artifacts:changes.view-diff',{name:single.display}) : tx('artifacts:changes.open-review')} aria-describedby={single ? pathId : undefined} onClick={()=>openReview(0)}>
 <span className="grid flex-none place-items-center box-border w-[40px] h-[40px] [border:0.5px_solid_var(--dsw-alias-border-l1)] rounded-[10px] bg-[color-mix(in_srgb,var(--dsw-static-neutral-00)_50%,transparent)] dark:bg-[color-mix(in_srgb,var(--dsw-static-neutral-00)_5%,transparent)]">{single ? <FileTypeIcon path={single.path} size={20}/> : <FileTypeIcon kind="code" size={20}/>}</span>
 <span className="flex flex-1 flex-col min-w-0"><span className="overflow-hidden text-[13px] font-medium leading-[20px] text-ellipsis whitespace-nowrap">{title}</span><span className="inline-flex text-[10px] leading-[16px] [color:var(--dsw-alias-label-tertiary)] [&[data-error=true]]:[color:var(--dsw-alias-state-error-primary)]"><span className="inline-flex gap-[6px] [font-family:var(--ds-font-family-code)] group-hover:hidden group-focus-visible:hidden">{counts(single ?? changes)}</span><span className="hidden group-hover:inline group-focus-visible:inline">{tx('artifacts:changes.open-review')}</span></span></span>
 </button>;
 const hover = (anchor:ReactNode,index:number) => <HoverCard anchor={anchor} content={preview(index)} previewAnchor={cardRef} copyLabel={tx('artifacts:changes.copy')} copiedLabel={tx('artifacts:changes.copied')}/>;
 return <div ref={cardRef} className="[--changes-fill:var(--dsw-static-neutral-50)] [--changes-hover:var(--dsw-static-neutral-100)] dark:[--changes-fill:var(--dsw-static-neutral-850)] dark:[--changes-hover:var(--dsw-static-neutral-800)] flex flex-col min-w-0 mt-[4px] overflow-hidden [border:0.5px_solid_var(--dsw-alias-border-l2)] rounded-[var(--dsw-radius-lg)] bg-[var(--dsw-alias-bg-layer-1)] [color:var(--dsw-alias-label-primary)] [&[data-single=true]]:[border-color:var(--dsw-alias-border-l1)] [&>span:not([hidden])]:block [&>span:not([hidden])]:w-full" data-changed-files data-single={!!single || undefined}>
 {single ? <>{hover(header,0)}<span id={pathId} hidden>{single.path}</span></> : header}
 {!single && <ul className="m-0 p-0 list-none [border-top:0.5px_solid_var(--dsw-alias-border-l2)] [&>li>span:not([hidden])]:block [&>li>span:not([hidden])]:w-full">{rows.map((file,index)=><li key={index}>
 {hover(<button type="button" className="flex items-center justify-between gap-[10px] box-border w-full min-w-0 min-h-[24px] [@media(pointer:coarse)]:min-h-[44px] m-0 pt-[7px] pr-[18px] pb-[7px] pl-[14px] border-0 bg-transparent [color:var(--dsw-alias-label-tertiary)] cursor-pointer [font-family:var(--ds-font-family-code)] text-[11px] leading-[18px] text-left hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)] focus-visible:bg-[var(--dsw-alias-interactive-bg-hover)] focus-visible:outline-none focus-visible:[box-shadow:inset_0_0_0_2px_var(--dsw-focus-ring-color,var(--dsw-alias-state-business-primary))] disabled:cursor-progress" aria-label={tx('artifacts:changes.view-diff',{name:file.display})} aria-describedby={`${pathId}-${index}`} onClick={()=>openReview(index)}>
 <span className="min-w-0 overflow-hidden [font-family:var(--dsw-font-family)] text-[12px] text-ellipsis whitespace-nowrap">{file.display}</span><span className="inline-flex flex-none gap-[6px] whitespace-nowrap [color:var(--dsw-alias-label-tertiary)] [&[data-error=true]]:[color:var(--dsw-alias-state-error-primary)]">{counts(file)}</span>
 </button>,index)}
 <span id={`${pathId}-${index}`} hidden>{file.path}</span>
 </li>)}</ul>}
 {changes.files.length>4 && <button type="button" className="inline-flex items-center justify-start gap-[4px] box-border w-full [@media(pointer:coarse)]:min-h-[44px] m-0 pt-[10px] pr-[18px] pb-[10px] pl-[14px] border-0 bg-transparent [color:var(--dsw-alias-label-tertiary)] cursor-pointer text-[12px] leading-[18px] text-left hover:bg-[var(--dsw-alias-interactive-bg-hover)] [&_svg]:flex-none [&_svg]:w-[14px] [&_svg]:h-[14px]" aria-expanded={expanded} onClick={()=>setExpanded(value=>!value)}>{tx(expanded?'artifacts:changes.collapse':'artifacts:changes.all',{count:changes.files.length})}<span style={expanded?{transform:'rotate(180deg)'}:undefined}><IconChevronDownOutline14/></span></button>}
 </div>;
}
