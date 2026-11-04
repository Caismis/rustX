import { AxesHelper, BoxHelper, Color, Group, Mesh, NeutralToneMapping, PerspectiveCamera, Raycaster, Scene, Vector2, Vector3, WebGLRenderer, HemisphereLight, DirectionalLight, type Object3D, type Box3 } from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import type { PreviewViewState } from '../../../../presentation/right-panel/preview-view-state';
import { GEOMETRY_LIMITS as L, GeometryError } from './limits';
import { createStructure } from './structure';
import { createAxisNavigation, type AxisLabels } from './axes';
import type { createOutlines, OutlineStatus } from './outlines';
export type { Part } from './structure';

type Saved = NonNullable<PreviewViewState['geometry']>;
export type StandardView = 'isometric' | 'front' | 'back' | 'left' | 'right' | 'top' | 'bottom';
let activeContexts = 0;

/** Mounted view owns every graphics resource. Source bytes belong to the content owner. */
export function mountGeometry(container: HTMLElement, object: Object3D, bounds: Box3, saved: Saved | undefined, labels: AxisLabels & { canvas: string }, change: (state: Omit<Saved, 'copyId'>) => void, lost: () => void) {
  if (activeContexts >= L.owners) throw new GeometryError('capacity');
  let renderer: WebGLRenderer;
  try { renderer = new WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'high-performance' }); }
  catch { throw new GeometryError('webgl'); }
  const context = renderer.getContext();
  if (!('fenceSync' in context)) { renderer.dispose(); renderer.forceContextLoss(); throw new GeometryError('webgl'); }
  const gl = context;
  renderer.toneMapping = NeutralToneMapping;
  activeContexts++;
  const canvas = renderer.domElement; canvas.tabIndex = 0; canvas.setAttribute('role', 'img'); canvas.setAttribute('aria-label', labels.canvas);
  let pendingDraw: WebGLSync | null = null;
  let outlines: ReturnType<typeof createOutlines> | undefined, outlineRequest = 0, outlinesWanted = saved?.outlines === true;
  container.append(canvas);
  const scene = new Scene(), centered = new Group(), camera = new PerspectiveCamera(45, 1, 0.001, 1000);
  const center = bounds.getCenter(new Vector3()), size = bounds.getSize(new Vector3()), radius = Math.max(size.length() / 2, 1e-6);
  centered.position.copy(center).negate(); centered.add(object); scene.add(centered);
  centered.updateMatrixWorld(true);
  scene.add(new HemisphereLight(0xffffff, 0x667080, 0.6));
  const light = new DirectionalLight(0xffffff, 3); light.position.set(-3, 6, 4); scene.add(light);
  const fill = new DirectionalLight(0xdce9ff, 1.5); fill.position.set(4, 2, -3); scene.add(fill);
  let controls = new OrbitControls(camera, canvas);
  const orbitUp = camera.up.clone();
  const configureControls = () => { controls.enableDamping = false; controls.minDistance = radius / 100; controls.maxDistance = radius * 100; controls.listenToKeyEvents(canvas); };
  configureControls();
  const axes = new AxesHelper(1), axisScene = new Scene(), axisCamera = new PerspectiveCamera(45, 1, 0.01, 10);
  const axisPositions = axes.geometry.getAttribute('position');
  axisPositions.setXYZ(0, -1, 0, 0); axisPositions.setXYZ(2, 0, -1, 0); axisPositions.setXYZ(4, 0, 0, -1);
  axisScene.add(axes); canvas.setAttribute('aria-description', labels.axes);
  const navigation = createAxisNavigation(container, labels, view => fit(view));
  const highlight = new BoxHelper(object, new Color(0xffbd45)); highlight.visible = false; scene.add(highlight);
  const structure = createStructure(object, center, saved);
  let frame = 0, disposed = false, initial = true, restoring = true;
  let cameraDirty = false, interacting = false, lastPublished = -Infinity, publication: ReturnType<typeof setTimeout> | undefined;
  const snapshot = (): Omit<Saved, 'copyId'> => ({ camera: { position: camera.position.toArray(), target: controls.target.toArray(), up: camera.up.toArray() }, ...structure.snapshot(), outlines: outlinesWanted });
  const emit = (publish = change) => {
    if (disposed || restoring) return;
    clearTimeout(publication); publication = undefined; cameraDirty = false; lastPublished = performance.now(); publish(snapshot());
  };
  const flush = (publish = change) => { if (cameraDirty) emit(publish); };
  const draw = () => {
    frame = 0; if (disposed) return;
    // At most one submitted frame per owner. Coalesce new camera poses while
    // the GPU is busy instead of accumulating obsolete full-resolution frames.
    if (pendingDraw) {
      const status = gl.clientWaitSync(pendingDraw, 0, 0);
      if (status === gl.TIMEOUT_EXPIRED) { frame = requestAnimationFrame(draw); return; }
      gl.deleteSync(pendingDraw); pendingDraw = null;
      if (status === gl.WAIT_FAILED) { dispose(); lost(); return; }
    }
    const { x: width, y: height } = renderer.getSize(new Vector2());
    renderer.setViewport(0, 0, width, height); renderer.autoClear = true; renderer.render(scene, camera);
    renderer.autoClear = false; renderer.clearDepth();
    axisCamera.position.copy(camera.position).sub(controls.target).normalize().multiplyScalar(3.5); axisCamera.up.copy(camera.up); axisCamera.lookAt(0, 0, 0);
    const side = Math.min(112, width, height); renderer.setViewport(8, 8, side, side); renderer.render(axisScene, axisCamera);
    navigation.update(axisCamera, side);
    pendingDraw = gl.fenceSync(gl.SYNC_GPU_COMMANDS_COMPLETE, 0);
    if (!pendingDraw) { dispose(); lost(); return; }
    gl.flush();
  };
  const render = () => { if (!disposed && !frame) frame = requestAnimationFrame(draw); };
  const updateHighlight = () => {
    const item = structure.selectedObject;
    highlight.visible = !!item;
    for (let ancestor = item; ancestor; ancestor = ancestor.parent ?? undefined) if (!ancestor.visible) highlight.visible = false;
    if (item && highlight.visible) highlight.setFromObject(item);
  };
  const select = (id?: string) => {
    if (!structure.select(id)) return;
    updateHighlight(); render(); emit();
  };
  const frameBounds = (box: Box3, view?: StandardView) => {
    if (box.isEmpty()) return;
    const directions: Record<StandardView, [number, number, number]> = { isometric: [1, 1, 1], front: [0, 0, 1], back: [0, 0, -1], left: [-1, 0, 0], right: [1, 0, 0], top: [0, 1, 0], bottom: [0, -1, 0] };
    const frameRadius = Math.max(box.getSize(new Vector3()).length() / 2, 1e-9);
    const distance = frameRadius / Math.sin(Math.PI / 8) / Math.min(1, camera.aspect) * 1.1;
    const direction = view ? new Vector3().fromArray(directions[view]) : camera.position.clone().sub(controls.target);
    const target = box.getCenter(new Vector3());
    camera.position.copy(direction).normalize().multiplyScalar(distance).add(target);
    if (view) camera.up.set(0, view === 'top' || view === 'bottom' ? 0 : 1, view === 'top' ? -1 : view === 'bottom' ? 1 : 0);
    controls.minDistance = frameRadius / 100;
    camera.near = Math.max(Math.min(radius, frameRadius) / 10_000, 1e-12); camera.far = radius * 1000; camera.updateProjectionMatrix(); synchronizeOrbit(target); render(); emit();
  };
  const fit = (view?: StandardView) => frameBounds(structure.bounds(object, true), view);
  const reset = () => frameBounds(bounds.clone().translate(center.clone().negate()), 'isometric');
  const focus = () => { if (structure.selectedObject) frameBounds(structure.bounds(structure.selectedObject, true)); };
  const visibility = (id: string, visible: boolean) => {
    if (!structure.visibility(id, visible)) return;
    updateHighlight(); render(); emit();
  };
  const structureAction = (action: () => boolean) => { if (action()) { updateHighlight(); render(); emit(); } };
  const setOutlines = async (enabled: boolean): Promise<OutlineStatus> => {
    const request = ++outlineRequest;
    if (disposed) return 'off'; outlinesWanted = enabled; emit();
    try {
      if (!outlines && !enabled) return 'off';
      if (!outlines) {
        const { createOutlines } = await import('./outlines');
        if (disposed || request !== outlineRequest) return 'off';
        outlines ??= createOutlines(object, () => getComputedStyle(container).getPropertyValue('--dsw-alias-label-document-preview').trim() || '#dbe6f3');
      }
      const status = await outlines.set(enabled);
      if (disposed || request !== outlineRequest) return 'off';
      outlinesWanted = status === 'on'; render(); emit(); return status;
    } catch {
      if (disposed || request !== outlineRequest) return 'off';
      outlinesWanted = false; emit(); return 'failed';
    }
  };
  const resize = () => {
    if (disposed) return;
    const width = Math.max(1, Math.min(container.clientWidth, 4096)), height = Math.max(1, Math.min(container.clientHeight, 4096));
    const pixelRatio = Math.min(globalThis.devicePixelRatio || 1, 2, Math.sqrt(L.canvasPixels / (width * height)));
    if (width === viewportWidth && height === viewportHeight && pixelRatio === viewportRatio) return;
    viewportWidth = width; viewportHeight = height; viewportRatio = pixelRatio;
    // Set size and DPR together: setPixelRatio() itself resizes the buffer.
    renderer.setDrawingBufferSize(width, height, pixelRatio);
    canvas.style.width = `${width}px`; canvas.style.height = `${height}px`;
    camera.aspect = width / height; camera.updateProjectionMatrix();
    if (initial) { initial = false; reset();
      const pose = saved?.camera;
      if (pose && [pose.position, pose.target, pose.up].every(values => values.length === 3 && values.every(value => Number.isFinite(value) && Math.abs(value) <= L.coordinate))) {
        const position = new Vector3().fromArray(pose.position), target = new Vector3().fromArray(pose.target), up = new Vector3().fromArray(pose.up);
        const distance = position.distanceTo(target);
        if (distance > 1e-12 && distance < radius * 1000 && up.lengthSq() > 0) { camera.position.copy(position); camera.up.copy(up).normalize(); controls.minDistance = Math.min(radius / 100, distance / 100); camera.near = Math.max(Math.min(radius, distance) / 10_000, 1e-12); camera.updateProjectionMatrix(); synchronizeOrbit(target); }
      }
    }
    render();
  };
  let viewportWidth = 0, viewportHeight = 0, viewportRatio = 0;
  const changed = () => {
    render(); if (restoring || disposed) return;
    cameraDirty = true;
    if (interacting) return;
    const remaining = 100 - (performance.now() - lastPublished);
    if (remaining <= 0) emit();
    else publication ??= setTimeout(() => { publication = undefined; flush(); }, remaining);
  };
  const started = () => { interacting = true; clearTimeout(publication); publication = undefined; };
  const ended = () => { interacting = false; flush(); };
  controls.addEventListener('change', changed);
  controls.addEventListener('start', started);
  controls.addEventListener('end', ended);
  function synchronizeOrbit(target: Vector3) {
    const minDistance = controls.minDistance;
    if (!orbitUp.equals(camera.up)) {
      // OrbitControls fixes its orbit-axis quaternion at construction time.
      // Recreate through its public lifecycle instead of mutating internals.
      controls.removeEventListener('change', changed); controls.removeEventListener('start', started); controls.removeEventListener('end', ended); controls.dispose();
      controls = new OrbitControls(camera, canvas); orbitUp.copy(camera.up); configureControls(); controls.addEventListener('change', changed); controls.addEventListener('start', started); controls.addEventListener('end', ended);
    }
    controls.minDistance = minDistance; controls.target.copy(target); controls.update();
  }
  const observer = new ResizeObserver(resize); observer.observe(container);
  // A resolution media query changes when moving between displays or zooming,
  // even when ResizeObserver sees no CSS-size change. Re-arm for the new DPR.
  let resolution: MediaQueryList;
  const resolutionChanged = () => { if (disposed) return; resolution.removeEventListener('change', resolutionChanged); watchResolution(); resize(); };
  const watchResolution = () => { resolution = matchMedia(`(resolution: ${globalThis.devicePixelRatio || 1}dppx)`); resolution.addEventListener('change', resolutionChanged); };
  watchResolution();
  const themeObserver = new MutationObserver(() => { if (!disposed && outlines?.enabled) { outlines.recolor(); render(); } });
  themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class', 'style'] });
  const raycaster = new Raycaster(); let down: [number, number] | undefined;
  const pointerDown = (event: PointerEvent) => { down = [event.clientX, event.clientY]; };
  const pointerUp = (event: PointerEvent) => {
    if (!down || Math.hypot(event.clientX - down[0], event.clientY - down[1]) > 4) return; down = undefined;
    const rect = canvas.getBoundingClientRect(); raycaster.setFromCamera(new Vector2((event.clientX - rect.left) / rect.width * 2 - 1, -(event.clientY - rect.top) / rect.height * 2 + 1), camera);
    const candidates: Mesh[] = [];
    object.traverseVisible(item => { if (item instanceof Mesh) candidates.push(item); });
    const hit = raycaster.intersectObjects(candidates, false)[0];
    select(hit ? structure.occurrenceIds.get(hit.object) : undefined);
  };
  const contextLost = (event: Event) => { event.preventDefault(); dispose(); lost(); };
  canvas.addEventListener('pointerdown', pointerDown); canvas.addEventListener('pointerup', pointerUp); canvas.addEventListener('webglcontextlost', contextLost);
  function dispose() {
    if (disposed) return; flush(); if (disposed) return;
    disposed = true; outlineRequest++; outlines?.dispose(); activeContexts--; clearTimeout(publication); cancelAnimationFrame(frame); observer.disconnect(); themeObserver.disconnect(); resolution.removeEventListener('change', resolutionChanged);
    canvas.removeEventListener('pointerdown', pointerDown); canvas.removeEventListener('pointerup', pointerUp); canvas.removeEventListener('webglcontextlost', contextLost);
    controls.removeEventListener('change', changed); controls.removeEventListener('start', started); controls.removeEventListener('end', ended); controls.dispose();
    highlight.geometry.dispose(); highlight.material.dispose(); axes.geometry.dispose();
    for (const material of Array.isArray(axes.material) ? axes.material : [axes.material]) material.dispose();
    if (pendingDraw) { gl.deleteSync(pendingDraw); pendingDraw = null; }
    navigation.dispose(); renderer.dispose(); renderer.forceContextLoss(); canvas.remove(); centered.remove(object);
  }
  try {
    resize();
    updateHighlight();
    restoring = false; emit();
  } catch (error) { dispose(); throw error; }
  return { parts: structure.parts, fit, reset, focus, select, visibility, inspect: structure.inspect,
    setOutlines,
    isolate: () => structureAction(structure.isolate), restore: () => structureAction(structure.restore), showAll: () => structureAction(structure.showAll),
    relabel: (value: AxisLabels & { canvas: string }) => { canvas.setAttribute('aria-label', value.canvas); canvas.setAttribute('aria-description', value.axes); navigation.relabel(value); }, flush, dispose };
}
