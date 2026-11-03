import { AxesHelper, BoxHelper, Color, Group, Mesh, NeutralToneMapping, PerspectiveCamera, Raycaster, Scene, Vector2, Vector3, WebGLRenderer, HemisphereLight, DirectionalLight, type Object3D, type Box3 } from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import type { PreviewViewState } from '../../../../presentation/right-panel/preview-view-state';
import { GEOMETRY_LIMITS as L, GeometryError } from './limits';

type Saved = NonNullable<PreviewViewState['geometry']>;
export type StandardView = 'isometric' | 'front' | 'back' | 'left' | 'right' | 'top' | 'bottom';
export interface Part { id: string; name: string; depth: number; kind: 'mesh' | 'group' }
let activeContexts = 0;

/** Mounted view owns every graphics resource. Source bytes belong to the content owner. */
export function mountGeometry(container: HTMLElement, object: Object3D, bounds: Box3, saved: Saved | undefined, labels: { canvas: string; axes: string }, change: (state: Omit<Saved, 'copyId'>) => void, lost: () => void) {
  if (activeContexts >= L.owners) throw new GeometryError('capacity');
  let renderer: WebGLRenderer;
  try { renderer = new WebGLRenderer({ antialias: false, alpha: true, powerPreference: 'high-performance' }); }
  catch { throw new GeometryError('webgl'); }
  renderer.toneMapping = NeutralToneMapping;
  activeContexts++;
  const canvas = renderer.domElement; canvas.tabIndex = 0; canvas.setAttribute('role', 'img'); canvas.setAttribute('aria-label', labels.canvas);
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
  axisScene.add(axes); canvas.setAttribute('aria-description', labels.axes);
  const highlight = new BoxHelper(object, new Color(0xffbd45)); highlight.visible = false; scene.add(highlight);
  const objects = new Map<string, Object3D>(), occurrenceIds = new Map<Object3D, string>(), parts: Part[] = [], hidden = new Set<string>();
  let selected: string | undefined, frame = 0, disposed = false, initial = true, restoring = true;
  let cameraDirty = false, lastPublished = -Infinity, publication: ReturnType<typeof setTimeout> | undefined;
  function walk(item: Object3D, id: string, depth: number) {
    if (item instanceof Mesh || item.name) { const name = typeof item.userData.name === 'string' ? item.userData.name : item.name; objects.set(id, item); occurrenceIds.set(item, id); parts.push({ id, name: name.slice(0, 256), depth, kind: item instanceof Mesh ? 'mesh' : 'group' }); }
    item.children.forEach((child, index) => walk(child, `${id}.${index}`, depth + 1));
  }
  walk(object, '0', 0);
  const snapshot = (): Omit<Saved, 'copyId'> => ({ camera: { position: camera.position.toArray(), target: controls.target.toArray(), up: camera.up.toArray() }, selected, hidden: [...hidden] });
  const emit = (publish = change) => {
    if (disposed || restoring) return;
    clearTimeout(publication); publication = undefined; cameraDirty = false; lastPublished = performance.now(); publish(snapshot());
  };
  const flush = (publish = change) => { if (cameraDirty) emit(publish); };
  const draw = () => {
    frame = 0; if (disposed) return;
    const { x: width, y: height } = renderer.getSize(new Vector2());
    renderer.setViewport(0, 0, width, height); renderer.autoClear = true; renderer.render(scene, camera);
    renderer.autoClear = false; renderer.clearDepth();
    axisCamera.position.copy(camera.position).sub(controls.target).normalize().multiplyScalar(3.5); axisCamera.up.copy(camera.up); axisCamera.lookAt(0, 0, 0);
    const side = Math.min(80, width, height); renderer.setViewport(8, 8, side, side); renderer.render(axisScene, axisCamera);
  };
  const render = () => { if (!disposed && !frame) frame = requestAnimationFrame(draw); };
  const updateHighlight = () => {
    const item = selected ? objects.get(selected) : undefined;
    highlight.visible = !!item;
    for (let ancestor = item; ancestor; ancestor = ancestor.parent ?? undefined) if (!ancestor.visible) highlight.visible = false;
    if (item && highlight.visible) highlight.setFromObject(item);
  };
  const select = (id?: string) => {
    const next = id && objects.has(id) ? id : undefined;
    if (next === selected) return;
    selected = next;
    updateHighlight(); render(); emit();
  };
  const fit = (view?: StandardView) => {
    const directions: Record<StandardView, [number, number, number]> = { isometric: [1, 1, 1], front: [0, 0, 1], back: [0, 0, -1], left: [-1, 0, 0], right: [1, 0, 0], top: [0, 1, 0], bottom: [0, -1, 0] };
    const distance = radius / Math.sin(Math.PI / 8) / Math.min(1, camera.aspect) * 1.1;
    const direction = view ? new Vector3().fromArray(directions[view]) : camera.position.clone().sub(controls.target);
    camera.position.copy(direction).normalize().multiplyScalar(distance);
    if (view) camera.up.set(0, view === 'top' || view === 'bottom' ? 0 : 1, view === 'top' ? -1 : view === 'bottom' ? 1 : 0);
    camera.near = Math.max(radius / 10_000, 1e-9); camera.far = radius * 1000; camera.updateProjectionMatrix(); synchronizeOrbit(new Vector3()); render(); emit();
  };
  const visibility = (id: string, visible: boolean) => {
    const item = objects.get(id); if (!item || item.visible === visible) return;
    item.visible = visible; if (visible) hidden.delete(id); else hidden.add(id);
    updateHighlight(); render(); emit();
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
    if (initial) { initial = false; fit('isometric');
      const pose = saved?.camera;
      if (pose && [pose.position, pose.target, pose.up].every(values => values.length === 3 && values.every(value => Number.isFinite(value) && Math.abs(value) <= L.coordinate))) {
        const position = new Vector3().fromArray(pose.position), target = new Vector3().fromArray(pose.target), up = new Vector3().fromArray(pose.up);
        if (position.distanceTo(target) > radius / 1000 && position.distanceTo(target) < radius * 1000 && up.lengthSq() > 0) { camera.position.copy(position); camera.up.copy(up).normalize(); synchronizeOrbit(target); }
      }
    }
    render();
  };
  let viewportWidth = 0, viewportHeight = 0, viewportRatio = 0;
  const changed = () => {
    render(); if (restoring || disposed) return;
    cameraDirty = true;
    const remaining = 100 - (performance.now() - lastPublished);
    if (remaining <= 0) emit();
    else publication ??= setTimeout(() => { publication = undefined; flush(); }, remaining);
  };
  const ended = () => flush();
  controls.addEventListener('change', changed);
  controls.addEventListener('end', ended);
  function synchronizeOrbit(target: Vector3) {
    if (!orbitUp.equals(camera.up)) {
      // OrbitControls fixes its orbit-axis quaternion at construction time.
      // Recreate through its public lifecycle instead of mutating internals.
      controls.removeEventListener('change', changed); controls.removeEventListener('end', ended); controls.dispose();
      controls = new OrbitControls(camera, canvas); orbitUp.copy(camera.up); configureControls(); controls.addEventListener('change', changed); controls.addEventListener('end', ended);
    }
    controls.target.copy(target); controls.update();
  }
  const observer = new ResizeObserver(resize); observer.observe(container);
  // A resolution media query changes when moving between displays or zooming,
  // even when ResizeObserver sees no CSS-size change. Re-arm for the new DPR.
  let resolution: MediaQueryList;
  const resolutionChanged = () => { if (disposed) return; resolution.removeEventListener('change', resolutionChanged); watchResolution(); resize(); };
  const watchResolution = () => { resolution = matchMedia(`(resolution: ${globalThis.devicePixelRatio || 1}dppx)`); resolution.addEventListener('change', resolutionChanged); };
  watchResolution();
  const raycaster = new Raycaster(); let down: [number, number] | undefined;
  const pointerDown = (event: PointerEvent) => { down = [event.clientX, event.clientY]; };
  const pointerUp = (event: PointerEvent) => {
    if (!down || Math.hypot(event.clientX - down[0], event.clientY - down[1]) > 4) return; down = undefined;
    const rect = canvas.getBoundingClientRect(); raycaster.setFromCamera(new Vector2((event.clientX - rect.left) / rect.width * 2 - 1, -(event.clientY - rect.top) / rect.height * 2 + 1), camera);
    const candidates: Mesh[] = [];
    object.traverseVisible(item => { if (item instanceof Mesh) candidates.push(item); });
    const hit = raycaster.intersectObjects(candidates, false)[0];
    select(hit ? occurrenceIds.get(hit.object) : undefined);
  };
  const contextLost = (event: Event) => { event.preventDefault(); dispose(); lost(); };
  canvas.addEventListener('pointerdown', pointerDown); canvas.addEventListener('pointerup', pointerUp); canvas.addEventListener('webglcontextlost', contextLost);
  function dispose() {
    if (disposed) return; flush(); if (disposed) return;
    disposed = true; activeContexts--; clearTimeout(publication); cancelAnimationFrame(frame); observer.disconnect(); resolution.removeEventListener('change', resolutionChanged);
    canvas.removeEventListener('pointerdown', pointerDown); canvas.removeEventListener('pointerup', pointerUp); canvas.removeEventListener('webglcontextlost', contextLost);
    controls.removeEventListener('change', changed); controls.removeEventListener('end', ended); controls.dispose();
    highlight.geometry.dispose(); highlight.material.dispose(); axes.geometry.dispose();
    for (const material of Array.isArray(axes.material) ? axes.material : [axes.material]) material.dispose();
    renderer.dispose(); renderer.forceContextLoss(); canvas.remove(); centered.remove(object);
  }
  try {
    resize();
    for (const id of (saved?.hidden ?? []).slice(0, L.nodes)) visibility(id, false);
    select(saved?.selected);
    restoring = false; emit();
  } catch (error) { dispose(); throw error; }
  return { parts, fit, select, visibility, flush, dispose };
}
