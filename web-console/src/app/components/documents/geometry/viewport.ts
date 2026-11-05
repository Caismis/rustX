import { AxesHelper, BoxHelper, BufferGeometry, BufferAttribute, Color, Group, Mesh, MeshStandardMaterial, NeutralToneMapping, OrthographicCamera, PerspectiveCamera, Raycaster, Scene, Vector2, Vector3, WebGLRenderer, HemisphereLight, DirectionalLight, type Object3D, type Box3 } from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import type { PreviewViewState } from '../../../../presentation/right-panel/preview-view-state';
import { GEOMETRY_LIMITS as L, GeometryError } from './limits';
import { createStructure } from './structure';
import { createAxisNavigation, type AxisLabels } from './axes';
import { createDisplayQuality } from './display-quality';
import { createDrawBatches } from './draw-batches';
import type { createOutlines, OutlineStatus } from './outlines';
export type { Part } from './structure';

type Saved = NonNullable<PreviewViewState['geometry']>;
export type Projection = NonNullable<Saved['camera']>['projection'];
export type StandardView = 'isometric' | 'front' | 'back' | 'left' | 'right' | 'top' | 'bottom';
let activeContexts = 0;
function studioLighting(scene: Scene) {
  scene.add(new HemisphereLight(0xffffff, 0x667080, .6));
  const light = new DirectionalLight(0xffffff, 3); light.position.set(-3, 6, 4); scene.add(light);
  const fill = new DirectionalLight(0xdce9ff, 1.5); fill.position.set(4, 2, -3); scene.add(fill);
}
/** Demand-owned context and shader preparation can overlap source import. No
 * model is drawn, copied or admitted by the trusted three-vertex warmup. */
export function prepareGeometryGraphics(generated: boolean, lost: () => void) {
  if (activeContexts >= L.owners) throw new GeometryError('capacity'); let renderer: WebGLRenderer;
  try { renderer = new WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'high-performance' }); }
  catch { throw new GeometryError('webgl'); }
  const gl = renderer.getContext();
  if (!('fenceSync' in gl)) { renderer.dispose(); renderer.forceContextLoss(); throw new GeometryError('webgl'); }
  activeContexts++; renderer.toneMapping = NeutralToneMapping;
  let disposed = false, warm: BufferGeometry | undefined, material: MeshStandardMaterial | undefined;
  const releaseWarm = () => { warm?.dispose(); material?.dispose(); warm = undefined; material = undefined; };
  const contextLost = (event: Event) => { event.preventDefault(); dispose(); lost(); };
  renderer.domElement.addEventListener('webglcontextlost', contextLost);
  function dispose() {
    if (disposed) return; disposed = true;
    renderer.domElement.removeEventListener('webglcontextlost', contextLost); releaseWarm();
    activeContexts--; renderer.dispose(); renderer.forceContextLoss();
  }
  try {
    if (generated) {
      const scene = new Scene(); studioLighting(scene);
      warm = new BufferGeometry().setAttribute('position', new BufferAttribute(new Float32Array([0,0,0, 1,0,0, 0,1,0]), 3)); warm.computeVertexNormals();
      material = new MeshStandardMaterial({ metalness: .05, roughness: .65 }); scene.add(new Mesh(warm, material));
      renderer.compile(scene, new PerspectiveCamera(45, 1, .001, 1000));
    }
  } catch { dispose(); throw new GeometryError('webgl'); }
  return { renderer, gl, dispose, adopt(scene: Scene, camera: PerspectiveCamera | OrthographicCamera) {
    renderer.domElement.removeEventListener('webglcontextlost', contextLost); renderer.compile(scene, camera); releaseWarm();
  } };
}

/** Mounted view owns every graphics resource. Source bytes belong to the content owner. */
export function mountGeometry(container: HTMLElement, object: Object3D, bounds: Box3, saved: Saved | undefined, labels: AxisLabels & { canvas: string }, change: (state: Omit<Saved, 'copyId'>) => void, lost: () => void, options?: { generated: boolean; pendingPreview?: boolean; onFirstGpuReady?: () => void; graphics?:ReturnType<typeof prepareGeometryGraphics> }) {
  const graphics = options?.graphics ?? prepareGeometryGraphics(options?.generated === true, lost), renderer = graphics.renderer, gl = graphics.gl;
  const canvas = renderer.domElement; canvas.tabIndex = 0; canvas.setAttribute('role', 'img'); canvas.setAttribute('aria-label', labels.canvas);
  let pendingDraw: WebGLSync | null = null;
  let presentationFrame = 0, firstSubmitted = false, firstReady = false;
  let outlines: ReturnType<typeof createOutlines> | undefined, outlineRequest = 0, outlinesWanted = saved?.outlines !== false;
  container.append(canvas);
  const scene = new Scene(), centered = new Group();
  let camera: PerspectiveCamera | OrthographicCamera = new PerspectiveCamera(45, 1, 0.001, 1000);
  const center = bounds.getCenter(new Vector3()), size = bounds.getSize(new Vector3()), radius = Math.max(size.length() / 2, 1e-6);
  centered.position.copy(center).negate(); centered.add(object); scene.add(centered);
  centered.updateMatrixWorld(true);
  studioLighting(scene);
  let controls: OrbitControls<PerspectiveCamera | OrthographicCamera> = new OrbitControls(camera, canvas);
  const orbitUp = camera.up.clone();
  const configureControls = () => { controls.enableDamping = false; controls.minDistance = radius / 100; controls.maxDistance = radius * 100; controls.listenToKeyEvents(canvas); };
  const configureZoom = () => {
    if (!(camera instanceof OrthographicCamera)) return;
    const height = camera.top - camera.bottom;
    controls.minZoom = height / (2 * Math.tan(Math.PI / 8) * controls.maxDistance);
    controls.maxZoom = height / (2 * Math.tan(Math.PI / 8) * Math.max(controls.minDistance, 1e-12));
  };
  configureControls();
  const axes = new AxesHelper(1), axisScene = new Scene(), axisCamera = new PerspectiveCamera(45, 1, 0.01, 10);
  const axisPositions = axes.geometry.getAttribute('position');
  axisPositions.setXYZ(0, -1, 0, 0); axisPositions.setXYZ(2, 0, -1, 0); axisPositions.setXYZ(4, 0, 0, -1);
  axisScene.add(axes); canvas.setAttribute('aria-description', labels.axes);
  const navigation = createAxisNavigation(container, labels, view => fit(view));
  const highlight = new BoxHelper(object, new Color(0xffbd45)); highlight.visible = false; scene.add(highlight);
  const structure = createStructure(object, center, saved);
  const quality = createDisplayQuality(object);
  const batches = createDrawBatches(object, options?.generated === true);
  let frame = 0, disposed = false, initial = true, restoring = true, inputDrawQueued = false;
  let moving = true, refinement: ReturnType<typeof setTimeout> | undefined, drawingRatio = 0, drawingWidth = 0, drawingHeight = 0;
  let backgroundPending=options?.pendingPreview===true;
  let releaseBackground: (() => void) | undefined;
  const backgroundDone = backgroundPending ? new Promise<void>(resolve => { releaseBackground = resolve; }) : Promise.resolve();
  let cameraDirty = false, interacting = false, lastPublished = -Infinity, publication: ReturnType<typeof setTimeout> | undefined;
  const projection = (): Projection => camera instanceof OrthographicCamera ? 'orthographic' : 'perspective';
  const verticalSpan = () => camera instanceof OrthographicCamera ? (camera.top - camera.bottom) / camera.zoom : 2 * camera.position.distanceTo(controls.target) * Math.tan(camera.fov * Math.PI / 360) / camera.zoom;
  const snapshot = (): Omit<Saved, 'copyId'> => ({ camera: { position: camera.position.toArray(), target: controls.target.toArray(), up: camera.up.toArray(), ...(camera instanceof OrthographicCamera ? { projection: 'orthographic', verticalSpan: verticalSpan() } : { projection: 'perspective' }) }, ...structure.snapshot(), outlines: outlinesWanted });
  const emit = (publish = change) => {
    if (disposed || restoring) return;
    clearTimeout(publication); publication = undefined; cameraDirty = false; lastPublished = performance.now(); rememberPose(); publish(snapshot());
  };
  const flush = (publish = change) => { if (!disposed && poseChanged()) cameraDirty = true; if (cameraDirty) emit(publish); };
  const draw = () => {
    frame = 0; if (disposed) return;
    // At most one submitted frame per owner. Coalesce new camera poses while
    // the GPU is busy instead of accumulating obsolete full-resolution frames.
    if (pendingDraw) {
      const status = gl.clientWaitSync(pendingDraw, 0, 0);
      if (status === gl.TIMEOUT_EXPIRED) { frame = requestAnimationFrame(draw); return; }
      gl.deleteSync(pendingDraw); pendingDraw = null;
      if (status === gl.WAIT_FAILED) { dispose(); lost(); return; }
      notifyFirstReady();
    }
    const width = viewportWidth, height = viewportHeight;
    const ratio = moving ? Math.min(viewportRatio, .5, Math.sqrt(L.movingPixels / (width * height))) : viewportRatio;
    if (drawingRatio !== ratio || drawingWidth !== width || drawingHeight !== height) {
      renderer.setDrawingBufferSize(width, height, ratio); drawingRatio = ratio; drawingWidth = width; drawingHeight = height;
    }
    renderer.setViewport(0, 0, width, height); renderer.autoClear = true;
    quality.draw(scene, moving, camera, height, () => batches.draw(() => renderer.render(scene, camera)));
    canvas.dataset.geometryQuality = moving ? 'interactive' : 'complete';
    renderer.autoClear = false; renderer.clearDepth();
    axisCamera.position.copy(camera.position).sub(controls.target).normalize().multiplyScalar(3.5); axisCamera.up.copy(camera.up); axisCamera.lookAt(0, 0, 0);
    const side = Math.min(112, width, height); renderer.setViewport(8, 8, side, side); renderer.render(axisScene, axisCamera);
    navigation.update(axisCamera, side);
    pendingDraw = gl.fenceSync(gl.SYNC_GPU_COMMANDS_COMPLETE, 0);
    if (!pendingDraw) { dispose(); lost(); return; }
    gl.flush();
    if (!firstSubmitted && options?.onFirstGpuReady) { firstSubmitted = true; presentationFrame = requestAnimationFrame(checkFirstReady); }
  };
  function notifyFirstReady() { if (firstSubmitted && !firstReady && !disposed) { firstReady = true; cancelAnimationFrame(presentationFrame); presentationFrame = 0; options?.onFirstGpuReady?.(); } }
  function checkFirstReady() {
    presentationFrame = 0; if (disposed || firstReady) return;
    if (!pendingDraw) { notifyFirstReady(); return; }
    const status = gl.clientWaitSync(pendingDraw, 0, 0);
    if (status === gl.TIMEOUT_EXPIRED) presentationFrame = requestAnimationFrame(checkFirstReady);
    else if (status === gl.WAIT_FAILED) { dispose(); lost(); }
    else notifyFirstReady();
  }
  const render = () => { if (!disposed && !frame) frame = requestAnimationFrame(draw); };
  const refine = () => {
    clearTimeout(refinement); refinement = undefined;
    if (disposed || interacting || !moving || backgroundPending) return;
    refinement = setTimeout(() => { refinement = undefined; if (!disposed) { moving = false; render(); } }, L.refineDelayMs);
  };
  const renderMoving = (urgent = false) => {
    moving = true; canvas.dataset.geometryQuality = 'interactive'; refine();
    if (!urgent) { render(); return; }
    if (inputDrawQueued) return; inputDrawQueued = true;
    queueMicrotask(() => { inputDrawQueued = false; if (disposed) return; cancelAnimationFrame(frame); frame = 0; draw(); });
  };
  const updateHighlight = () => {
    const item = structure.selectedObject;
    highlight.visible = !!item;
    for (let ancestor = item; ancestor; ancestor = ancestor.parent ?? undefined) if (!ancestor.visible) highlight.visible = false;
    if (item && highlight.visible) highlight.setFromObject(item);
  };
  const select = (id?: string) => {
    if (!structure.select(id)) return;
    updateHighlight(); renderMoving(); emit();
  };
  const updateClipping = () => {
    const far = Math.max(radius * 1000, camera.position.length() + radius * 2, camera.position.distanceTo(controls.target) + radius * 2);
    if (camera.far !== far) { camera.far = far; camera.updateProjectionMatrix(); }
  };
  const setOrthographicSpan = (span: number) => {
    if (!(camera instanceof OrthographicCamera)) return;
    const aspect = viewportWidth / viewportHeight || 1;
    camera.top = span / 2; camera.bottom = -span / 2; camera.left = -span * aspect / 2; camera.right = span * aspect / 2; camera.zoom = 1;
  };
  const setProjection = (value: Projection) => {
    if (disposed || value === projection()) return;
    const target = controls.target.clone(), direction = camera.position.clone().sub(target), up = camera.up.clone(), span = verticalSpan();
    const position = camera.position.clone(), near = camera.near, far = camera.far;
    if (value === 'orthographic') { camera = new OrthographicCamera(); setOrthographicSpan(span); }
    else { camera = new PerspectiveCamera(45, viewportWidth / viewportHeight || 1); position.copy(direction).normalize().multiplyScalar(span / (2 * Math.tan(Math.PI / 8))).add(target); }
    camera.position.copy(position); camera.up.copy(up); camera.near = near; camera.far = far; camera.updateProjectionMatrix();
    controls.maxDistance = Math.max(controls.maxDistance, camera.position.distanceTo(target));
    interacting = false; synchronizeOrbit(target); updateClipping(); renderMoving(); emit();
  };
  const frameBounds = (box: Box3, view?: StandardView) => {
    if (box.isEmpty()) return;
    const directions: Record<StandardView, [number, number, number]> = { isometric: [1, 1, 1], front: [0, 0, 1], back: [0, 0, -1], left: [-1, 0, 0], right: [1, 0, 0], top: [0, 1, 0], bottom: [0, -1, 0] };
    const frameRadius = Math.max(box.getSize(new Vector3()).length() / 2, 1e-9);
    const aspect = viewportWidth / viewportHeight || 1, distance = frameRadius / Math.sin(Math.PI / 8) / Math.min(1, aspect) * 1.1;
    const direction = view ? new Vector3().fromArray(directions[view]) : camera.position.clone().sub(controls.target);
    const target = box.getCenter(new Vector3());
    camera.position.copy(direction).normalize().multiplyScalar(distance).add(target);
    if (view) camera.up.set(0, view === 'top' || view === 'bottom' ? 0 : 1, view === 'top' ? -1 : view === 'bottom' ? 1 : 0);
    if (camera instanceof OrthographicCamera) setOrthographicSpan(frameRadius * 2 / Math.min(1, aspect) * 1.1);
    controls.minDistance = frameRadius / 100; controls.maxDistance = Math.max(radius * 100, distance);
    camera.near = Math.max(Math.min(radius, frameRadius) / 10_000, 1e-12); camera.updateProjectionMatrix(); synchronizeOrbit(target); updateClipping(); renderMoving(); emit();
  };
  const fit = (view?: StandardView) => frameBounds(structure.bounds(object, true), view);
  const reset = () => frameBounds(bounds.clone().translate(center.clone().negate()), 'isometric');
  const focus = () => { if (structure.selectedObject) frameBounds(structure.bounds(structure.selectedObject, true)); };
  const visibility = (id: string, visible: boolean) => {
    if (!structure.visibility(id, visible)) return;
    outlines?.sync(); updateHighlight(); renderMoving(); emit();
  };
  const structureAction = (action: () => boolean) => { if (action()) { outlines?.sync(); updateHighlight(); renderMoving(); emit(); } };
  const setOutlines = async (enabled: boolean): Promise<OutlineStatus> => {
    const request = ++outlineRequest;
    if (disposed) return 'off'; outlinesWanted = enabled; emit();
    try {
      if (!outlines && !enabled) return 'off';
      if (enabled && backgroundPending) await backgroundDone;
      if (disposed || request !== outlineRequest) return 'off';
      if (!outlines) {
        const { createOutlines } = await import('./outlines');
        if (disposed || request !== outlineRequest) return 'off';
        outlines ??= createOutlines(object, () => getComputedStyle(container).getPropertyValue('--dsw-alias-label-document-preview').trim() || '#dbe6f3', options);
        centered.add(outlines.group);
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
    // The next draw applies the latest size, DPR and display quality together.
    // Do not allocate a full-resolution buffer that the first coarse draw discards.
    canvas.style.width = `${width}px`; canvas.style.height = `${height}px`;
    if (camera instanceof PerspectiveCamera) camera.aspect = width / height;
    else { const span = camera.top - camera.bottom; camera.left = -span * width / height / 2; camera.right = span * width / height / 2; }
    camera.updateProjectionMatrix();
    if (initial) { initial = false; reset();
      const pose = saved?.camera, cameraLimit = Math.max(L.coordinate, radius * 2000);
      const vector = (values: unknown, limit: number): values is [number, number, number] => Array.isArray(values) && values.length === 3 && values.every(value => Number.isFinite(value) && Math.abs(value) <= limit);
      if (pose && (pose.projection === 'perspective' || pose.projection === 'orthographic') && Number.isFinite(cameraLimit) && vector(pose.position, cameraLimit) && vector(pose.target, cameraLimit) && vector(pose.up, L.coordinate)) {
        const position = new Vector3().fromArray(pose.position), target = new Vector3().fromArray(pose.target), up = new Vector3().fromArray(pose.up);
        const distance = position.distanceTo(target);
        const spanDistance = pose.projection === 'orthographic' ? (typeof pose.verticalSpan === 'number' && Number.isFinite(pose.verticalSpan) ? pose.verticalSpan / (2 * Math.tan(Math.PI / 8)) : NaN) : distance;
        if (distance > 1e-12 && distance < radius * 1000 && up.lengthSq() > 0 && Number.isFinite(spanDistance) && spanDistance >= 1e-12 && spanDistance < radius * 1000) {
          if (pose.projection === 'orthographic') { camera = new OrthographicCamera(); setOrthographicSpan(pose.verticalSpan); }
          camera.position.copy(position); camera.up.copy(up).normalize(); controls.minDistance = Math.min(radius / 100, distance / 100, spanDistance / 100); controls.maxDistance = Math.max(radius * 100, distance, spanDistance);
          camera.near = Math.max(Math.min(radius, distance, spanDistance) / 10_000, 1e-12); camera.updateProjectionMatrix(); synchronizeOrbit(target); updateClipping();
        }
      }
    }
    renderMoving();
  };
  let viewportWidth = 0, viewportHeight = 0, viewportRatio = 0;
  const observedPosition = camera.position.clone(), observedTarget = controls.target.clone(), observedRotation = camera.quaternion.clone();
  let observedZoom = camera.zoom, observedProjection = projection();
  const rememberPose = () => { observedPosition.copy(camera.position); observedTarget.copy(controls.target); observedRotation.copy(camera.quaternion); observedZoom = camera.zoom; observedProjection = projection(); };
  const poseChanged = () => !observedPosition.equals(camera.position) || !observedTarget.equals(controls.target) || !observedRotation.equals(camera.quaternion) || observedZoom !== camera.zoom || observedProjection !== projection();
  const changed = () => {
    rememberPose();
    updateClipping(); renderMoving(true); if (restoring || disposed) return;
    cameraDirty = true;
    if (interacting) return;
    const remaining = 100 - (performance.now() - lastPublished);
    if (remaining <= 0) emit();
    else publication ??= setTimeout(() => { publication = undefined; flush(); }, remaining);
  };
  const observePose = () => {
    if (!disposed && poseChanged()) changed();
  };
  // OrbitControls' change event uses an absolute world-distance threshold.
  // Its public input events still update smaller models. Read the final pose
  // after their handlers, without scheduling any work while the view is idle.
  const inputFinished = () => { queueMicrotask(observePose); };
  const started = () => { interacting = true; clearTimeout(refinement); refinement = undefined; observePose(); clearTimeout(publication); publication = undefined; };
  const ended = () => { observePose(); interacting = false; refine(); flush(); };
  const inputMoved = () => { if (interacting) inputFinished(); };
  canvas.ownerDocument.addEventListener('pointermove', inputMoved, true);
  canvas.addEventListener('keydown', inputFinished, true);
  controls.addEventListener('change', changed);
  controls.addEventListener('start', started);
  controls.addEventListener('end', ended);
  function synchronizeOrbit(target: Vector3) {
    const minDistance = controls.minDistance, maxDistance = controls.maxDistance;
    if (controls.object !== camera || !orbitUp.equals(camera.up)) {
      // OrbitControls fixes its orbit-axis quaternion at construction time.
      // Recreate through its public lifecycle instead of mutating internals.
      controls.removeEventListener('change', changed); controls.removeEventListener('start', started); controls.removeEventListener('end', ended); controls.dispose();
      // Its constructor updates around the default origin target. Preserve the
      // intended pose until the retained target and limits can be applied below.
      const position = camera.position.clone(), rotation = camera.quaternion.clone(), zoom = camera.zoom;
      controls = new OrbitControls(camera, canvas);
      camera.position.copy(position); camera.quaternion.copy(rotation); camera.zoom = zoom; camera.updateProjectionMatrix();
      orbitUp.copy(camera.up); configureControls(); controls.addEventListener('change', changed); controls.addEventListener('start', started); controls.addEventListener('end', ended);
    }
    controls.minDistance = minDistance; controls.maxDistance = maxDistance; configureZoom(); controls.target.copy(target);
    // The caller renders and publishes this owner-driven pose exactly once.
    // Keep OrbitControls' change listener for subsequent user input only.
    controls.removeEventListener('change', changed);
    try { controls.update(); } finally { controls.addEventListener('change', changed); }
    rememberPose();
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
    const hit = quality.pick(raycaster, candidates);
    select(hit ? structure.occurrenceIds.get(hit.object) : undefined);
  };
  const contextLost = (event: Event) => { event.preventDefault(); dispose(); lost(); };
  canvas.addEventListener('pointerdown', pointerDown); canvas.addEventListener('pointerup', pointerUp); canvas.addEventListener('webglcontextlost', contextLost);
  function dispose() {
    if (disposed) return; flush(); if (disposed) return;
    disposed = true; outlineRequest++; releaseBackground?.(); releaseBackground = undefined; outlines?.dispose(); quality.dispose(); batches.dispose(); clearTimeout(refinement); clearTimeout(publication); cancelAnimationFrame(frame); cancelAnimationFrame(presentationFrame); observer.disconnect(); themeObserver.disconnect(); resolution.removeEventListener('change', resolutionChanged);
    canvas.removeEventListener('pointerdown', pointerDown); canvas.removeEventListener('pointerup', pointerUp); canvas.removeEventListener('webglcontextlost', contextLost);
    canvas.ownerDocument.removeEventListener('pointermove', inputMoved, true); canvas.removeEventListener('keydown', inputFinished, true);
    controls.removeEventListener('change', changed); controls.removeEventListener('start', started); controls.removeEventListener('end', ended); controls.dispose();
    highlight.geometry.dispose(); highlight.material.dispose(); axes.geometry.dispose();
    for (const material of Array.isArray(axes.material) ? axes.material : [axes.material]) material.dispose();
    if (pendingDraw) { gl.deleteSync(pendingDraw); pendingDraw = null; }
    navigation.dispose(); graphics.dispose(); canvas.remove(); centered.remove(object);
  }
  try {
    graphics.adopt(scene,camera);
    resize();
    updateHighlight();
    restoring = false; emit();
  } catch (error) { dispose(); throw error; }
  return { parts: structure.parts, fit, reset, focus, select, visibility, inspect: structure.inspect,
    setOutlines, setProjection, refreshPreview: () => { backgroundPending=false;releaseBackground?.();releaseBackground=undefined;quality.refresh();if(moving){refine();render();} }, get projection() { return projection(); },
    isolate: () => structureAction(structure.isolate), restore: () => structureAction(structure.restore), showAll: () => structureAction(structure.showAll),
    relabel: (value: AxisLabels & { canvas: string }) => { canvas.setAttribute('aria-label', value.canvas); canvas.setAttribute('aria-description', value.axes); navigation.relabel(value); }, flush, dispose };
}
