import type { StandardView } from './viewport';

/** Faces and arrows identify opposite directions without relying on color. */
export function GeometryViewGlyph({ view }: { view: StandardView }) {
  const faces = { top: '8,1.8 13.5,4.9 8,8.1 2.5,4.9', left: '2.5,4.9 8,8.1 8,14.2 2.5,11.1', front: '8,8.1 13.5,4.9 13.5,11.1 8,14.2' };
  const face = view === 'bottom' ? 'top' : view === 'right' ? 'left' : view === 'back' ? 'front' : view;
  const opposite = view === 'bottom' || view === 'right' || view === 'back';
  return <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" aria-hidden="true" data-view-icon={view}>
    {face !== 'isometric' && <polygon points={faces[face]} fill="currentColor" fillOpacity={opposite ? '.12' : '.4'} strokeDasharray={opposite ? '1.3 1' : undefined}/>}
    <path d="m8 1.8 5.5 3.1v6.2L8 14.2l-5.5-3.1V4.9L8 1.8Z"/><path d="m2.5 4.9 5.5 3.2 5.5-3.2M8 8.1v6.1"/>
    {opposite && <path d="M10.7 2.1h3.2v3.2m0-3.2-3.6 3.6" strokeWidth="1.5"/>}
  </svg>;
}
