/* Copyright (c) 2026 DeepSeek. MIT. MarkdownFileLink delegate port. */
import { createContext, useContext, type ReactNode } from 'react';
import { LinkIconMedium, classifyLinkPath } from '../primitives/LinkIcon';
import { parseFileLink } from './file-link';
import css from './MarkdownText.module.css';

export type FileReference = { path: string; line?: number };

export interface MarkdownFileMentions {
  resolve(value: string): {
    open: () => void;
    label: string;
    title: string;
  } | undefined;
}

export const FileLinks = createContext<((file: FileReference) => void) | undefined>(undefined);
export const FileMentionsContext = createContext<MarkdownFileMentions | undefined>(undefined);

function pathBasename(path: string): string {
  const normalized = path.replace(/\\/g, '/').replace(/\/+$/, '');
  const slash = normalized.lastIndexOf('/');
  return slash >= 0 ? normalized.slice(slash + 1) : normalized;
}

/**
 * Resolves inline-code tokens against one turn's produced or delivered paths.
 * A token resolves only when every exact or basename match identifies one unique target.
 */
export function producedFileMentions(
  paths: readonly string[],
  openFile: (file: FileReference) => void,
  label: (path: string) => string,
): MarkdownFileMentions {
  return {
    resolve(value) {
      const matches = new Set(paths.filter(path => path === value || pathBasename(path) === value));
      const path = matches.size === 1 ? matches.values().next().value : undefined;
      if (path === undefined) return undefined;
      return { open: () => { openFile({ path }); }, label: label(path), title: path };
    },
  };
}

export function FileLink({ url, streaming, children, fallback }: { url: string; streaming: boolean; children: ReactNode; fallback: ReactNode }) {
  const open = useContext(FileLinks), file = streaming ? undefined : parseFileLink(url);
  if (!file) return fallback;
  if (!open) return children;
  return <button type="button" className={`${css.fileMention} ${css.fileLink}`} title={file.path} onClick={() => open(file)}><LinkIconMedium kind={classifyLinkPath(file.path)} className={css.linkIcon}/>{children}</button>;
}
