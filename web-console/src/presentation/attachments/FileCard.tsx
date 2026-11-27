/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
import { fileExtension, FileTypeIcon } from '../primitives/FileTypeIcon'
import { fileSizeText } from '../primitives/file-size'
import { IconCloseFill14 as IconCloseFillRegular } from '../primitives/icons'

/** Localized strings consumed by one pending-file card. */
export interface FileCardLabels {
 /** Card body announcement, e.g. "Pending file {name}". */
 readonly label: string
 /** Remove-button label. */
 readonly remove: string
 /** Status line while the upload is in flight. */
 readonly uploading: string
 /** Status line and retry affordance after a failed upload. */
 readonly failed: string
 /** Retry-button label. */
 readonly retry: string
}

/** Upload display state resolved by the owner. */
export type FileCardState = 'uploading' | 'ready' | 'error'

/** One pending file card: type glyph, name, size or upload status, remove, retry. */
export function FileCard({
 name, bytes, state, progress, labels, onRemove, onRetry, status,
}: {
 name: string
 bytes: number
 state: FileCardState
 progress?: number
 labels: FileCardLabels
 onRemove?: () => void
 onRetry?: () => void
 status?: string
}) {
 const extension = fileExtension(name).toUpperCase().slice(0, 8)
 const meta = status ?? (state === 'uploading'
 ? labels.uploading
 : state === 'error'
 ? labels.failed
 : [extension, fileSizeText(bytes)].filter(part => part !== '').join(' '))
 const retryable = state === 'error' && onRetry !== undefined
 return (
 <div
 className={`group/filecard relative inline-flex items-center gap-[10px] w-[240px] h-[64px] px-[12px] py-0 [border:0.5px_solid_var(--dsw-alias-border-l2,rgba(0,0,0,0.12))] rounded-[var(--dsw-radius-lg)] bg-[var(--dsw-specific-input-major,transparent)] box-border text-left${state === 'error' ? ' [border-color:var(--dsw-alias-state-error-primary,#d54941)]!' : ''}`}
 title={name}
 data-attachment-state={state}
 aria-busy={state === 'uploading'}
 >
 <span className="inline-flex flex-none items-center justify-center w-[28px] h-[28px]" aria-hidden>
 {state === 'uploading'
 ? <span className="w-[20px] h-[20px] [border:2px_solid_currentColor] [border-top-color:transparent] rounded-full [corner-shape:round]! animate-upload-spin" />
 : <FileTypeIcon path={name} />}
 </span>
 {retryable
 ? (
 <button type="button" className="flex flex-1 flex-col min-w-0 px-0 py-[8px] border-0 bg-transparent [color:inherit] text-left cursor-pointer" aria-label={labels.retry} onClick={onRetry}>
 <span className="overflow-hidden whitespace-nowrap text-ellipsis [color:var(--dsw-alias-label-primary)] text-[14px] font-medium leading-[22px] group-hover/filecard:pr-[18px] group-focus-within/filecard:pr-[18px]">{name}</span>
 <span className="overflow-hidden whitespace-nowrap text-ellipsis [color:var(--dsw-alias-label-tertiary,rgba(0,0,0,0.45))] text-[12px] leading-[15px] [color:var(--dsw-alias-state-error-primary,#d54941)]!">{meta}</span>
 </button>
 )
 : (
 <span className="flex flex-1 flex-col min-w-0 px-0 py-[8px]" aria-label={labels.label}>
 <span className="overflow-hidden whitespace-nowrap text-ellipsis [color:var(--dsw-alias-label-primary)] text-[14px] font-medium leading-[22px] group-hover/filecard:pr-[18px] group-focus-within/filecard:pr-[18px]">{name}</span>
 <span className={state === 'error' ? 'overflow-hidden whitespace-nowrap text-ellipsis [color:var(--dsw-alias-label-tertiary,rgba(0,0,0,0.45))] text-[12px] leading-[15px] [color:var(--dsw-alias-state-error-primary,#d54941)]!' : 'overflow-hidden whitespace-nowrap text-ellipsis [color:var(--dsw-alias-label-tertiary,rgba(0,0,0,0.45))] text-[12px] leading-[15px]'}>{meta}</span>
 </span>
 )}
 {onRemove && <button
 type="button"
 className={state === 'error' ? 'absolute top-[6px] right-[6px] inline-flex items-center justify-center w-[18px] h-[18px] p-0 border-none rounded-full [corner-shape:round]! bg-[var(--dsw-alias-button-contrast-fill,rgba(0,0,0,0.72))] [color:var(--dsw-alias-label-primary-inverted,#fff)] opacity-0 cursor-pointer transition-opacity duration-200 motion-reduce:transition-none group-hover/filecard:opacity-100 focus-visible:opacity-100 [@media(pointer:coarse)]:opacity-100 opacity-100! bg-[var(--dsw-alias-state-error-primary,#d54941)]! [color:#fff]!' : 'absolute top-[6px] right-[6px] inline-flex items-center justify-center w-[18px] h-[18px] p-0 border-none rounded-full [corner-shape:round]! bg-[var(--dsw-alias-button-contrast-fill,rgba(0,0,0,0.72))] [color:var(--dsw-alias-label-primary-inverted,#fff)] opacity-0 cursor-pointer transition-opacity duration-200 motion-reduce:transition-none group-hover/filecard:opacity-100 focus-visible:opacity-100 [@media(pointer:coarse)]:opacity-100'}
 aria-label={labels.remove}
 onClick={onRemove}
 >
 <IconCloseFillRegular size={12} />
 </button>}
 {state === 'uploading' && (
 <span className="absolute right-[12px] bottom-[5px] left-[12px] overflow-hidden h-[2px] rounded-[1px] bg-[var(--dsw-alias-fill-tertiary,rgba(0,0,0,0.08))]" aria-hidden>
 <span
 className="block w-[35%] h-full rounded-[inherit] bg-[var(--dsw-alias-brand-primary,#4d6bfe)] [&:not([style])]:animate-file-card-progress"
 style={progress === undefined ? undefined : { width: `${String(Math.min(1, Math.max(0, progress)) * 100)}%` }}
 />
 </span>
 )}
 </div>
 )
}
