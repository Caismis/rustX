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
export const IconHome = glyph(<path d="M4 10.5 12 4l8 6.5V20h-5v-5H9v5H4z" />);
export const IconPencil = glyph(<path d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4" />);
export const IconBraces = glyph(<path d="M8 3H6a2 2 0 0 0-2 2v4a3 3 0 0 1-2 3 3 3 0 0 1 2 3v4a2 2 0 0 0 2 2h2M16 3h2a2 2 0 0 1 2 2v4a3 3 0 0 0 2 3 3 3 0 0 0-2 3v4a2 2 0 0 1-2 2h-2" />);

const KIND_ICONS = {
  start: IconEntry,
  agent: glyph(<path d="M4 8h16v12H4zM12 3v5M9 3h6M8.5 13h.1M15.5 13h.1M9 17h6" />),
  model: glyph(<><rect x="5" y="5" width="14" height="14" rx="3" /><path d="M9 9h6v6H9zM9 2v3M15 2v3M9 19v3M15 19v3M2 9h3M2 15h3M19 9h3M19 15h3" /></>),
  tool: glyph(<path d="m5 16 8-8M8 5l4-2 6 6-2 4M3 19l2 2 7-7" />),
  branch: glyph(<><path d="M7 4v9a5 5 0 0 0 5 5h5M7 13a5 5 0 0 1 5-5h5" /><circle cx="7" cy="4" r="2" /><circle cx="17" cy="8" r="2" /><circle cx="17" cy="18" r="2" /></>),
  review: glyph(<path d="M4 4h16v13H9l-5 4zM8 9h8M8 13h5" />),
  loop: glyph(<path d="M19 7h-9a6 6 0 0 0 0 12h8M15 4l4 3-4 3M7 17l3 2-3 2" />),
  iteration: glyph(<path d="M4 6h9M4 12h9M4 18h6M17 8a4 4 0 0 1 0 8h-1M18 13l-2 3 2 3" />),
  loop_end: glyph(<><circle cx="12" cy="12" r="9" /><rect x="9" y="9" width="6" height="6" rx="1" /></>),
  assigner: glyph(<path d="M4 9h7M4 15h7M15 6l5 12M20 6l-5 12" />),
  return: glyph(<path d="M20 6v7a5 5 0 0 1-5 5H5m5-5-5 5 5 5" />),
} satisfies Record<NodeKind | 'start', ReturnType<typeof glyph>>;

export function KindIcon({ kind, size, className }: { kind: NodeKind | 'start'; size?: number; className?: string }) {
  const Icon = KIND_ICONS[kind];
  return <Icon size={size} className={className} />;
}

/** Builtin Tools by name, as Dify gives each tool its provider's icon; any
 * other Tool shows the Tool kind's glyph. */
const TOOL_ICONS: Record<string, ReturnType<typeof glyph>> = {
  ask_user: glyph(<><circle cx="12" cy="12" r="9" /><path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.6.3-1 .9-1 1.6v.6M12 17h.01" /></>),
  read: glyph(<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12zM12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6z" />),
  read_image: glyph(<><rect x="3" y="4" width="18" height="16" rx="2" /><circle cx="9" cy="10" r="1.6" /><path d="m21 16-5-5-9 9" /></>),
  present: glyph(<path d="M4 4h16v11H4zM12 15v5M8 20h8M8 9h8" />),
  write: glyph(<path d="M6 2.5h9l5 5v13a1 1 0 0 1-1 1H6a2 2 0 0 1-2-2v-15a2 2 0 0 1 2-2zM15 2.5V8h5M12 11v6M9 14h6" />),
  edit: glyph(<path d="M4 20h4L19 9l-4-4L4 16zM13.5 6.5l4 4" />),
  glob: IconFolder,
  grep: glyph(<><circle cx="10.5" cy="10.5" r="6.5" /><path d="m20 20-4.9-4.9M8 10.5h5" /></>),
  bash: glyph(<><rect x="3" y="4" width="18" height="16" rx="2" /><path d="m7 9 3 3-3 3M12.5 15H17" /></>),
};

export function ToolIcon({ name, size, className }: { name: string; size?: number; className?: string }) {
  const Icon = Object.hasOwn(TOOL_ICONS, name) ? TOOL_ICONS[name] : KIND_ICONS.tool;
  return <Icon size={size} className={className} />;
}

const SOURCE_ICONS = {
  // A plug: one connected MCP server.
  mcp: glyph(<path d="M9 3v5M15 3v5M6 8h12v3a6 6 0 0 1-12 0zM12 17v4" />),
  // Two interlocked halves: one Managed Python package.
  python: glyph(<path d="M12 3c-4 0-4 1.6-4 3v2h4.5M8 8H6c-2 0-3 1.6-3 4s1 4 3 4h2v-2.5a2.5 2.5 0 0 1 2.5-2.5h3A2.5 2.5 0 0 0 16 8.5V6c0-1.4-1-3-4-3zM16 10h2c2 0 3 1.6 3 4s-1 4-3 4h-2M12 21c4 0 4-1.6 4-3v-2h-4.5" />),
};

export function SourceIcon({ kind, size, className }: { kind: keyof typeof SOURCE_ICONS; size?: number; className?: string }) {
  const Icon = SOURCE_ICONS[kind];
  return <Icon size={size} className={className} />;
}
