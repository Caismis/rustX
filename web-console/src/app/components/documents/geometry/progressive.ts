import { Box3, Group, Vector3 } from 'three';
import type { PreviewViewState } from '../../../../presentation/right-panel/preview-view-state';
import type { GeometryRoughPose } from '../../../../client/geometry-rough';
import type { GeometryPartial } from './parse';
import { mountGeometry, type Projection, type prepareGeometryGraphics } from './viewport';
import { GEOMETRY_LIMITS as L, GeometryError } from './limits';

type Saved = NonNullable<PreviewViewState['geometry']>;
type Camera = NonNullable<Saved['camera']>;
export interface GeometryBoundsHint { min: [number, number, number]; max: [number, number, number] }
function framing(bounds: Box3, container: HTMLElement) {
  const size = bounds.getSize(new Vector3()), radius = Math.max(size.length() / 2, 1e-6);
  const aspect = Math.max(1, container.clientWidth) / Math.max(1, container.clientHeight);
  return { span: Math.max(size.x, size.y, size.z), distance: radius / Math.sin(Math.PI / 8) / Math.min(1, aspect) * 1.1, height: radius * 2 / Math.min(1, aspect) * 1.1 };
}
export function cameraFromPose(pose: GeometryRoughPose, bounds: Box3, container: HTMLElement, projection: Projection = 'perspective'): Camera {
  const frame = framing(bounds, container), target = new Vector3().fromArray(pose.target).multiplyScalar(frame.span);
  const position = target.clone().add(new Vector3().fromArray(pose.direction).normalize().multiplyScalar(frame.distance / pose.zoom));
  return { position: position.toArray(), target: target.toArray(), up: pose.up ?? [0,1,0], ...(projection === 'orthographic' ? { projection, verticalSpan: frame.height / pose.zoom } : { projection }) };
}
function normalizedPose(camera: Camera, bounds: Box3, container: HTMLElement) {
  const frame = framing(bounds, container), target = new Vector3().fromArray(camera.target), direction = new Vector3().fromArray(camera.position).sub(target);
  const zoom = camera.projection === 'orthographic' ? frame.height / camera.verticalSpan : frame.distance / direction.length();
  return { direction: direction.normalize().toArray(), target: target.divideScalar(frame.span).toArray(), up: camera.up, zoom } satisfies GeometryRoughPose;
}

/** Owns only the provisional graph. All fragments are independently admitted
 * by the receiver. Completed content reuses the same renderer/context; the
 * temporary mesh/material resources retire after its shaders are adopted. */
export function progressivePresentation(options: {
  container: HTMLElement; graphics: ReturnType<typeof prepareGeometryGraphics>; current: () => boolean;
  copyId: string;
  labels: () => Parameters<typeof mountGeometry>[4]; saved?: Saved;
  bounds?: () => GeometryBoundsHint | undefined; pose?: () => GeometryRoughPose | undefined;
  mounted: (view: ReturnType<typeof mountGeometry> | undefined) => void;
  displayed: () => void; failed: (error: unknown) => void;
}) {
  const root = new Group(), parts: GeometryPartial[] = [];
  let bounds: Box3 | undefined, view: ReturnType<typeof mountGeometry> | undefined, camera: Camera | undefined;
  let frame = 0, closed = false, accepting = true, touched = false, dragging = false, lastInput = -Infinity;
  let quiet: ReturnType<typeof setTimeout> | undefined, settle: (() => void) | undefined;
  const arm = () => {
    clearTimeout(quiet); quiet = undefined;
    if (settle && !dragging) quiet = setTimeout(() => { quiet = undefined; settle?.(); settle = undefined; }, Math.max(0, L.refineDelayMs - (performance.now() - lastInput)));
  };
  const activity = (active: boolean) => { touched = true; dragging = active; lastInput = performance.now(); arm(); };
  const stopFrame = () => { cancelAnimationFrame(frame); frame = 0; };
  const draw = () => {
    frame = 0; if (closed || !accepting || !options.current()) return;
    try {
      if (view) { view.refreshContent(); return; }
      // Prefix meshes do not yet have final occurrence IDs. Only camera state
      // can be carried into this provisional graph.
      const seed = options.pose?.(), saved: Saved = { copyId: options.copyId, camera: options.saved?.camera, outlines: false };
      if (seed) saved.camera = cameraFromPose(seed, bounds!, options.container, saved.camera?.projection);
      view = mountGeometry(options.container, root, bounds!, saved, options.labels(), patch => { camera = patch.camera; }, () => options.failed(new GeometryError('webgl')), {
        generated: true, graphics: options.graphics, provisional: true, onInteraction: activity,
        onFirstGpuReady: () => { if (!closed && options.current()) options.displayed(); },
      });
      options.mounted(view);
    } catch (error) { options.failed(error); }
  };
  return {
    add(part: GeometryPartial) {
      if (closed || !accepting || !options.current()) { part.dispose(); return; }
      if (parts.length >= L.partialUpdates) { part.dispose(); throw new GeometryError('limit'); }
      parts.push(part); root.add(part.scene);
      if (!bounds) {
        bounds = part.bounds.clone(); const hint = options.bounds?.();
        if (hint && [...hint.min,...hint.max].every(value => Number.isFinite(value) && Math.abs(value) <= L.coordinate)) bounds.union(new Box3(new Vector3().fromArray(hint.min),new Vector3().fromArray(hint.max)));
      } else if (!view) bounds.union(part.bounds);
      if (!frame) frame = requestAnimationFrame(draw);
    },
    async complete() {
      accepting = false; stopFrame();
      if (view && touched && (dragging || performance.now() - lastInput < L.refineDelayMs)) await new Promise<void>(resolve => { settle = resolve; arm(); });
      if (closed || !options.current()) return;
      view?.flush(patch => { camera = patch.camera; });
      const seed = touched && camera && bounds ? { pose: normalizedPose(camera,bounds,options.container), projection: camera.projection } : undefined;
      view?.handoff(); view = undefined; options.mounted(undefined);
      return seed;
    },
    dispose() {
      if (closed) return; closed = true; accepting = false; stopFrame(); clearTimeout(quiet); settle?.(); settle = undefined;
      view?.dispose(); view = undefined; parts.splice(0).forEach(part => part.dispose()); root.clear();
    },
  };
}
