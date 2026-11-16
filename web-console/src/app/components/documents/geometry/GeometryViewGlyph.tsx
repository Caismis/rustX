import type { StandardView } from './viewport';

type Point = readonly [number, number];
const vertex = {
  T: [8, 1.8], R: [13.5, 4.9], C: [8, 8], L: [2.5, 4.9],
  H: [8, 8], Rb: [13.5, 11.1], B: [8, 14.2], Lb: [2.5, 11.1],
} as const satisfies Record<string, Point>;
const faces = {
  top: ['T', 'R', 'C', 'L'], bottom: ['H', 'Rb', 'B', 'Lb'],
  left: ['L', 'T', 'H', 'Lb'], right: ['C', 'R', 'Rb', 'B'],
  front: ['L', 'C', 'B', 'Lb'], back: ['T', 'R', 'Rb', 'H'],
} as const satisfies Record<Exclude<StandardView, 'isometric'>, readonly (keyof typeof vertex)[]>;

/** The isometric view and view toolbar share this original cube glyph. */
export function GeometryCubeGlyph() {
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" aria-hidden="true">
    <path d="m8 1.8 5.5 3.1v6.2L8 14.2l-5.5-3.1V4.9L8 1.8Z"/><path d="m2.5 4.9 5.5 3.2 5.5-3.2M8 8.1v6.1"/>
  </svg>;
}

/** Each direction highlights its actual cube face; the rear edges are dashed. */
export function GeometryViewGlyph({ view }: { view: StandardView }) {
  if (view === 'isometric') return <GeometryCubeGlyph/>;
  const points = faces[view].map(name => vertex[name].join(',')).join(' ');
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" aria-hidden="true" data-view-icon={view}>
    <polygon points={points} fill="currentColor" fillOpacity=".32" stroke="none"/>
    <path d="M8 1.8 13.5 4.9 13.5 11.1 8 14.2 2.5 11.1 2.5 4.9 8 1.8Z M2.5 4.9 8 8 13.5 4.9 M8 8 8 14.2"/>
    <path d="M8 8 8 1.8 M8 8 13.5 11.1 M8 8 2.5 11.1" strokeDasharray="1.3 1"/>
  </svg>;
}
