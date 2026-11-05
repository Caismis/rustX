import { Box3, Group, Vector3 } from 'three';
import type { PreviewViewState } from '../../../../presentation/right-panel/preview-view-state';
import type { GeometryRoughPose } from '../../../../client/geometry-rough';
import type { GeometryLod } from './parse';
import { mountGeometry, type Projection, type prepareGeometryGraphics } from './viewport';
import { GEOMETRY_LIMITS as L, GeometryError } from './limits';

type Saved = NonNullable<PreviewViewState['geometry']>;
type Camera = NonNullable<Saved['camera']>;
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
function normalizedPose(camera: Camera, bounds: Box3, container: HTMLElement, presentationCenter: Vector3) {
  const frame = framing(bounds, container), target = new Vector3().fromArray(camera.target), direction = new Vector3().fromArray(camera.position).sub(target);
  const zoom = camera.projection === 'orthographic' ? frame.height / camera.verticalSpan : frame.distance / direction.length();
  target.add(presentationCenter).sub(bounds.getCenter(new Vector3()));
  return { direction: direction.normalize().toArray(), target: target.divideScalar(frame.span).toArray(), up: camera.up, zoom } satisfies GeometryRoughPose;
}

/** Replaces independently admitted whole-model tiers in one provisional graph.
 * Completed content reuses its renderer, canvas and camera interaction owner. */
export function progressivePresentation(options: {
  container: HTMLElement; graphics: ReturnType<typeof prepareGeometryGraphics>; current: () => boolean;
  copyId: string;
  labels: () => Parameters<typeof mountGeometry>[4]; saved?: Saved;
  pose?: () => GeometryRoughPose | undefined;
  mounted: (view: ReturnType<typeof mountGeometry> | undefined) => void;
  displayed: () => void; failed: (error: unknown) => void;
}) {
  const root = new Group();
  const presentationCenter = new Vector3();
  let active: GeometryLod | undefined, pending: GeometryLod | undefined, lastLevel = -1;
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
    const next = pending; pending = undefined;
    if (!next) return;
    const previous = active; active = next; bounds = next.bounds;
    root.clear(); root.add(next.scene);
    try {
      if (view) { view.refreshContent(bounds, next.level); return; }
      bounds.getCenter(presentationCenter);
      // Tier meshes do not supply authoritative selection/visibility metadata.
      const seed = options.pose?.(), saved: Saved = { copyId: options.copyId, camera: options.saved?.camera, outlines: false };
      if (seed) saved.camera = cameraFromPose(seed, bounds, options.container, saved.camera?.projection);
      view = mountGeometry(options.container, root, bounds, saved, options.labels(), patch => { camera = patch.camera; }, () => options.failed(new GeometryError('webgl')), {
        generated: true, graphics: options.graphics, provisional: true, lod: next.level, onInteraction: activity,
        onFirstGpuReady: () => { if (!closed && options.current()) options.displayed(); },
      });
      options.mounted(view);
    } catch (error) { options.failed(error); }
    finally { previous?.dispose(); }
  };
  return {
    add(lod: GeometryLod) {
      if (closed || !accepting || !options.current()) { lod.dispose(); return; }
      if ((lod.level !== 0 && lod.level !== 1) || lod.level <= lastLevel) { lod.dispose(); throw new GeometryError('invalid'); }
      lastLevel = lod.level; pending?.dispose(); pending = lod;
      if (!frame) frame = requestAnimationFrame(draw);
    },
    async complete() {
      accepting = false; stopFrame(); pending?.dispose(); pending = undefined;
      if (view && touched && (dragging || performance.now() - lastInput < L.refineDelayMs)) await new Promise<void>(resolve => { settle = resolve; arm(); });
      if (closed || !options.current()) return;
      view?.flush(patch => { camera = patch.camera; });
      const seed = touched && camera && bounds ? { pose: normalizedPose(camera,bounds,options.container,presentationCenter), projection: camera.projection } : undefined;
      view?.handoff(); view = undefined; options.mounted(undefined);
      return seed;
    },
    dispose() {
      if (closed) return; closed = true; accepting = false; stopFrame(); clearTimeout(quiet); settle?.(); settle = undefined;
      view?.dispose(); view = undefined; pending?.dispose(); pending = undefined; active?.dispose(); active = undefined; root.clear();
    },
  };
}
