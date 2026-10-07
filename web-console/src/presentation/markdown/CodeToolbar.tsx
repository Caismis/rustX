/* Copyright (c) 2026 DeepSeek. MIT. CodeToolbar adapted for an owner-controlled wrap preference; see PROVENANCE.md. */
import { IconCheckOutline14, IconCopyOutline16 } from '../primitives/icons';
import { Tooltip } from '../primitives/Tooltip';
import { supportsHighlighting } from './highlight';
import css from './CodeToolbar.module.css';
export function CodeToolbar({ lang, label, copied, copyLabel, copiedLabel, onCopy }: { lang?: string | undefined; label: string; copied: boolean; copyLabel: string; copiedLabel: string; onCopy: () => void }) {
  const clipboardLabel = copied ? copiedLabel : copyLabel;
  return <div className={css.header} data-code-block-banner><div className={css.heading}><span className={css.language}>{supportsHighlighting(lang) ? lang : label}</span></div><div className={css.actions}><Tooltip label={clipboardLabel} side="top"><button type="button" className={css.action} aria-label={clipboardLabel} onClick={onCopy}>{copied ? <IconCheckOutline14/> : <IconCopyOutline16 size={14}/>}</button></Tooltip></div></div>;
}
