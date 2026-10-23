import type { ReactNode } from 'react';
import type { NodeKind } from '../document/model';

/** Compact stroked glyphs of the Workflow surface, riding currentColor.
 * Shapes follow the owner-approved Workflow design reference (Issue #491). */
function glyph(paths: ReactNode) {
  return ({ size = 16, className }: { size?: number; className?: string }) => (
    <svg width={size} height={size} className={className} viewBox="0 0 24 24" fill="none" stroke="currentColor"
      strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths}</svg>
  );
}

export const IconWorkflow = glyph(<><rect x="3" y="3" width="6" height="6" rx="1.5" /><rect x="15" y="3" width="6" height="6" rx="1.5" /><rect x="9" y="15" width="6" height="6" rx="1.5" /><path d="M6 9v4h6v2M18 9v4h-6" /></>);
export const IconFile = glyph(<path d="M6 2.5h9l5 5v13a1 1 0 0 1-1 1H6a2 2 0 0 1-2-2v-15a2 2 0 0 1 2-2zM15 2.5V8h5M8 13h8M8 17h7" />);
export const IconFolder = glyph(<path d="M3.5 7a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" />);
export const IconGrid = glyph(<><rect x="3" y="3" width="7" height="7" rx="1.5" /><rect x="14" y="3" width="7" height="7" rx="1.5" /><rect x="3" y="14" width="7" height="7" rx="1.5" /><rect x="14" y="14" width="7" height="7" rx="1.5" /></>);
export const IconList = glyph(<path d="M4 6h2M10 6h10M4 12h2M10 12h10M4 18h2M10 18h10" />);
export const IconArrowLeft = glyph(<path d="m14 5-7 7 7 7M8 12h13" />);
export const IconExplain = glyph(<path d="M5 5h14v14H5zM9 10h6M9 14h4" />);
export const IconCursor = glyph(<path d="M4 3 18 12l-7 1.5L8 21z" />);
export const IconHand = glyph(<path d="M9 13V6a2 2 0 0 1 4 0v6-4a2 2 0 0 1 4 0v5m0-2a2 2 0 0 1 4 0v4a7 7 0 0 1-7 7h-2c-2 0-3.5-.5-5-2l-3-3c-1-1-1-2.7 0-3.5s2-1 3 0l2 1" />);
export const IconFit = glyph(<path d="M4 9V4h5M15 4h5v5M4 15v5h5M20 15v5h-5" />);
export const IconMinus = glyph(<path d="M5 12h14" />);
export const IconPlus = glyph(<path d="M12 5v14M5 12h14" />);
export const IconUndo = glyph(<path d="M9 14 4 9l5-5M4 9h10a6 6 0 0 1 0 12h-3" />);
export const IconRedo = glyph(<path d="m15 14 5-5-5-5M20 9H10a6 6 0 0 0 0 12h3" />);
export const IconBlock = glyph(<><rect x="3.5" y="3.5" width="17" height="17" rx="3" /><path d="M8 9h8M8 13h5" /></>);
export const IconEntry = glyph(<path d="M3 12h11M10 7l5 5-5 5M18 4v16" />);

const KIND_ICONS = {
  agent: glyph(<path d="M4 8h16v12H4zM12 3v5M9 3h6M8.5 13h.1M15.5 13h.1M9 17h6" />),
  tool: glyph(<path d="m5 16 8-8M8 5l4-2 6 6-2 4M3 19l2 2 7-7" />),
  branch: glyph(<><path d="M7 4v9a5 5 0 0 0 5 5h5M7 13a5 5 0 0 1 5-5h5" /><circle cx="7" cy="4" r="2" /><circle cx="17" cy="8" r="2" /><circle cx="17" cy="18" r="2" /></>),
  parallel: glyph(<path d="M7 4v16M17 4v16M5 7h4M15 7h4M5 17h4M15 17h4" />),
  review: glyph(<path d="M4 4h16v13H9l-5 4zM8 9h8M8 13h5" />),
  loop: glyph(<path d="M19 7h-9a6 6 0 0 0 0 12h8M15 4l4 3-4 3M7 17l3 2-3 2" />),
  return: glyph(<path d="M20 6v7a5 5 0 0 1-5 5H5m5-5-5 5 5 5" />),
} satisfies Record<NodeKind, ReturnType<typeof glyph>>;

export function KindIcon({ kind, size, className }: { kind: NodeKind; size?: number; className?: string }) {
  const Icon = KIND_ICONS[kind];
  return <Icon size={size} className={className} />;
}
