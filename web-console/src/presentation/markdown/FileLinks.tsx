/* Copyright (c) 2026 DeepSeek. MIT. MarkdownFileLink delegate port. */
import { createContext, useContext, type ReactNode } from 'react';
import { LinkIconMedium, classifyLinkPath } from '../primitives/LinkIcon';
import { parseFileLink } from './file-link';
import css from './MarkdownText.module.css';
export type FileReference = { path: string; line?: number };
export const FileLinks = createContext<((file: FileReference) => void) | undefined>(undefined);
export function FileLink({ url, streaming, children, fallback }: { url: string; streaming: boolean; children: ReactNode; fallback: ReactNode }) {
  const open = useContext(FileLinks), file = streaming ? undefined : parseFileLink(url);
  if (!file) return fallback;
  if (!open) return children;
  return <button type="button" className={`${css.fileMention} ${css.fileLink}`} title={file.path} onClick={() => open(file)}><LinkIconMedium kind={classifyLinkPath(file.path)} className={css.linkIcon}/>{children}</button>;
}
