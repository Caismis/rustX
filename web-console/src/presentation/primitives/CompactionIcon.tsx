/* Copyright (c) 2026 DeepSeek. MIT. Adapted from pinned Harness icons/artwork.ts; see PROVENANCE.md. */
import type { IconProps } from './icons/props';

/** Compaction glyphs at the pinned reference's regular stroke and view box. */
export function CompactionIcon({ kind, size = 14, className }: IconProps & { kind: 'context' | 'expanded' | 'collapsed' }) {
  const path = kind === 'context' ? 'M3 4L7 8L3 12M9 12H13'
    : kind === 'expanded' ? 'M4 6L7.29289 9.29289C7.68342 9.68342 8.31658 9.68342 8.70711 9.29289L12 6'
      : 'M6 12L9.29289 8.70711C9.68342 8.31658 9.68342 7.68342 9.29289 7.29289L6 4';
  return <svg width={size} height={size} className={className} viewBox="0 0 16 16" fill="none" strokeWidth="1" aria-hidden="true"><path d={path} stroke="currentColor"/></svg>;
}
