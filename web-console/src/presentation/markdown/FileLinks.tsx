/* Copyright (c) 2026 DeepSeek. MIT. MarkdownFileLink delegate port. */
import { createContext, useContext, type ReactNode } from 'react';
import { LinkIconMedium, classifyLinkPath } from '../primitives/LinkIcon';
import { parseFileLink } from './file-link';
import { markdownTextCss } from './markdownTextCss';
export type FileReference = { path: string; line?: number };
export const FileLinks = createContext<((file: FileReference) => void) | undefined>(undefined);
export function FileLink({ url, streaming, children, fallback }: { url: string; streaming: boolean; children: ReactNode; fallback: ReactNode }) {
  const open = useContext(FileLinks), file = streaming ? undefined : parseFileLink(url);
  if (!file) return fallback;
  if (!open) return children;
  return <button type="button" className={`${markdownTextCss.fileMention} ${markdownTextCss.fileLink}`} title={file.path} onClick={() => open(file)}><LinkIconMedium kind={classifyLinkPath(file.path)} className={markdownTextCss.linkIcon}/>{children}</button>;
}
