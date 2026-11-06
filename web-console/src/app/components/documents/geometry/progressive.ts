import { Box3, Group, Vector3 } from 'three';
import type { PreviewViewState } from '../../../../presentation/right-panel/preview-view-state';
import type { GeometryLod } from './parse';
import { mountGeometry, type Projection, type prepareGeometryGraphics } from './viewport';
import { GEOMETRY_LIMITS as L, GeometryError } from './limits';

export interface GeometryPose { direction: [number, number, number]; target: [number, number, number]; zoom: number; up?: [number, number, number] }

type Saved = NonNullable<PreviewViewState['geometry']>;
type Camera = NonNullable<Saved['camera']>;
function framing(bounds: Box3, container: Pick<HTMLElement, 'clientWidth' | 'clientHeight'>) {
  const size = bounds.getSize(new Vector3()), radius = Math.max(size.length() / 2, 1e-6);
  const aspect = Math.max(1, container.clientWidth) / Math.max(1, container.clientHeight);
  return { span: Math.max(size.x, size.y, size.z), distance: radius / Math.sin(Math.PI / 8) / Math.min(1, aspect) * 1.1, height: radius * 2 / Math.min(1, aspect) * 1.1 };
}
export function cameraFromPose(pose: GeometryPose, bounds: Box3, container: Pick<HTMLElement, 'clientWidth' | 'clientHeight'>, projection: Projection = 'orthographic'): Camera {
  const frame = framing(bounds, container), target = new Vector3().fromArray(pose.target).multiplyScalar(frame.span);
  const position = target.clone().add(new Vector3().fromArray(pose.direction).normalize().multiplyScalar(frame.distance / pose.zoom));
  return { position: position.toArray(), target: target.toArray(), up: pose.up ?? [0,1,0], ...(projection === 'orthographic' ? { projection, verticalSpan: frame.height / pose.zoom } : { projection }) };
}
function normalizedPose(camera: Camera, bounds: Box3, container: Pick<HTMLElement, 'clientWidth' | 'clientHeight'>, presentationCenter: Vector3) {
  const frame = framing(bounds, container), target = new Vector3().fromArray(camera.target), direction = new Vector3().fromArray(camera.position).sub(target);
  const zoom = camera.projection === 'orthographic' ? frame.height / camera.verticalSpan : frame.distance / direction.length();
  target.add(presentationCenter).sub(bounds.getCenter(new Vector3()));
  return { direction: direction.normalize().toArray(), target: target.divideScalar(frame.span).toArray(), up: camera.up, zoom } satisfies GeometryPose;
}

/** Replaces independently admitted whole-model tiers in one provisional graph.
 * Completed content reuses its renderer, canvas and camera interaction owner. */
export function progressivePresentation(options: {
  container: HTMLElement; graphics: ReturnType<typeof prepareGeometryGraphics>; current: () => boolean;
  copyId: string;
  visible?: boolean;
  labels: () => Parameters<typeof mountGeometry>[4]; saved?: Saved;
  mounted: (view: ReturnType<typeof mountGeometry> | undefined) => void;
  displayed: () => void; failed: (error: unknown) => void;
}) {
  const root = new Group();
  const presentationCenter = new Vector3();
  let active: GeometryLod | undefined, pending: GeometryLod | undefined, lastLevel = -1;
  let bounds: Box3 | undefined, view: ReturnType<typeof mountGeometry> | undefined, camera: Camera | undefined;
  let frame = 0, closed = false, accepting = true, touched = false, dragging = false, lastInput = -Infinity;
  let visible = options.visible !== false;
  let quiet: ReturnType<typeof setTimeout> | undefined, settle: (() => void) | undefined;
  const arm = () => {
    clearTimeout(quiet); quiet = undefined;
    if (settle && visible && !dragging) quiet = setTimeout(() => { quiet = undefined; settle?.(); settle = undefined; }, Math.max(0, L.refineDelayMs - (performance.now() - lastInput)));
  };
  const activity = (active: boolean) => { if (!visible) return; touched = true; dragging = active; lastInput = performance.now(); arm(); };
  const stopFrame = () => { cancelAnimationFrame(frame); frame = 0; };
  const draw = () => {
    frame = 0; if (closed || !visible || !accepting || !options.current()) return;
    const next = pending; pending = undefined;
    if (!next) return;
    const previous = active; active = next; bounds = next.bounds;
    root.clear(); root.add(next.scene);
    try {
      if (view) { view.refreshContent(bounds, next.level); return; }
      bounds.getCenter(presentationCenter);
      // Tier meshes do not supply authoritative selection/visibility metadata.
      const saved: Saved = { copyId: options.copyId, camera: options.saved?.camera, navigationMode: options.saved?.navigationMode, outlines: false };
      view = mountGeometry(options.container, root, bounds, saved, options.labels(), patch => { camera = patch.camera; }, () => options.failed(new GeometryError('webgl')), {
        generated: true, visible, graphics: options.graphics, provisional: true, lod: next.level, onInteraction: activity,
        onFirstGpuReady: () => { if (!closed && options.current()) options.displayed(); },
      });
      options.mounted(view);
    } catch (error) { options.failed(error); }
    finally { previous?.dispose(); }
  };
  return {
    setVisible(value: boolean) {
      if (closed || value === visible) return;
      visible = value;
      if (!value) {
        stopFrame(); clearTimeout(quiet); quiet = undefined; dragging = false;
        settle?.(); settle = undefined;
      }
      view?.setVisible(value);
      if (value && accepting && pending && !frame) frame = requestAnimationFrame(draw);
    },
    add(lod: GeometryLod) {
      if (closed || !accepting || !options.current()) { lod.dispose(); return; }
      if ((lod.level !== 0 && lod.level !== 1) || lod.level <= lastLevel) { lod.dispose(); throw new GeometryError('invalid'); }
      lastLevel = lod.level; pending?.dispose(); pending = lod;
      if (visible && !frame) frame = requestAnimationFrame(draw);
    },
    async complete() {
      accepting = false; stopFrame(); pending?.dispose(); pending = undefined;
      if (visible && view && touched && (dragging || performance.now() - lastInput < L.refineDelayMs)) await new Promise<void>(resolve => { settle = resolve; arm(); });
      if (closed || !options.current()) return;
      view?.flush(patch => { camera = patch.camera; });
      const presentationSize = view?.viewportSize ?? { clientWidth: options.container.clientWidth, clientHeight: options.container.clientHeight };
      const seed = { ...(touched && camera && bounds ? { pose: normalizedPose(camera,bounds,presentationSize,presentationCenter), projection: camera.projection } : {}), viewportSize: presentationSize, focusedAxis: view?.focusedAxis, navigationMode: view?.navigationMode };
      const modeChanged = view && view.navigationMode !== (options.saved?.navigationMode === 'pan' ? 'pan' : 'orbit');
      view?.handoff(); view = undefined; options.mounted(undefined);
      return seed.pose || seed.focusedAxis || modeChanged ? seed : undefined;
    },
    dispose() {
      if (closed) return; closed = true; accepting = false; stopFrame(); clearTimeout(quiet); settle?.(); settle = undefined;
      view?.dispose(); view = undefined; pending?.dispose(); pending = undefined; active?.dispose(); active = undefined; root.clear();
    },
  };
}
