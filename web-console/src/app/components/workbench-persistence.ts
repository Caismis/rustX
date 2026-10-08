/* Copyright (c) 2026 DeepSeek. MIT. Source port; see PROVENANCE.md. */
/** Validated current-layout snapshots; undo history belongs to the live window. */
import { EMPTY_HISTORY, type History } from '../../presentation/dockkit/engine/sequence';
import type { LayoutState } from '../../presentation/dockkit/contract/types';
export type SurfaceState = {layout:LayoutState;minted:number;history:History};

/** Persistence namespace shared by scoped stores and startup discovery. */
export const sidebarPersistence = 'rustx.workbench-layout.v1'

const paneId = /^(?:pane|float)[1-9][0-9]*$/u;
const splitId = /^split[1-9][0-9]*$/u;
const tabId = /^tab[1-9][0-9]*$/u;
const record = (value: unknown): value is Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value);
const id = (value: unknown, pattern: RegExp): value is string => typeof value === 'string' && pattern.test(value);
const nodeId = (value: unknown) => id(value, paneId) || id(value, splitId);
const finite = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);
const positive = (value: unknown) => finite(value) && value > 0;

// This application deliberately has no schema framework. Validate the source
// layout contract at the storage boundary before its reference graph is used.
function parseSurface(value: unknown): Pick<SurfaceState, 'layout' | 'minted'> {
  const reject: () => never = () => { throw new Error('Invalid saved sidebar layout shape'); };
  if (!record(value) || !Number.isSafeInteger(value.minted) || (value.minted as number) < 0 || !record(value.layout)) return reject();
  const layout = value.layout;
  if (!record(layout.nodes) || !record(layout.tabs) || !nodeId(layout.rootId) || !id(layout.activePaneId, paneId)
    || !Array.isArray(layout.floats) || !layout.floats.every(item => id(item, paneId))
    || typeof layout.expanded !== 'boolean' || !(layout.mode === 'push' || layout.mode === 'fullscreen')) return reject();
  for (const [key, node] of Object.entries(layout.nodes)) {
    if (!nodeId(key) || !record(node)) return reject();
    if (node.kind === 'pane') {
      if (!id(node.id, paneId) || !(node.host === 'dock' || node.host === 'float')
        || !Array.isArray(node.tabs) || !node.tabs.every(item => id(item, tabId))
        || (node.activeTabId !== undefined && !id(node.activeTabId, tabId))) return reject();
      if (node.rect !== undefined && (!record(node.rect) || !finite(node.rect.x) || !finite(node.rect.y)
        || !positive(node.rect.width) || !positive(node.rect.height))) return reject();
    } else if (node.kind === 'split') {
      if (!id(node.id, splitId) || !(node.axis === 'row' || node.axis === 'column')
        || !Array.isArray(node.children) || node.children.length < 2 || !node.children.every(nodeId)
        || !Array.isArray(node.sizes) || node.sizes.length < 2 || !node.sizes.every(positive)) return reject();
    } else return reject();
  }
  for (const [key, tab] of Object.entries(layout.tabs)) {
    if (!id(key, tabId) || !record(tab) || !id(tab.id, tabId)
      || !(typeof tab.kind === 'string' && ['start', 'files', 'file', 'terminal'].includes(tab.kind))
      || typeof tab.contentId !== 'string' || !tab.contentId || typeof tab.title !== 'string') return reject();
  }
  return value as unknown as Pick<SurfaceState, 'layout' | 'minted'>;
}

function validateReferences(value: Pick<SurfaceState, 'layout' | 'minted'>): void {
  const { layout, minted } = value
  const reject: () => never = () => { throw new Error('Invalid saved sidebar layout references') }
  const visited = new Set<string>()
  const usedTabs = new Set<string>()
  const pending = [{ id: layout.rootId, host: 'dock' }, ...layout.floats.map(id => ({ id, host: 'float' }))]
  for (const [id, entry] of [...Object.entries(layout.nodes), ...Object.entries(layout.tabs)]) {
    if (entry.id !== id || Number(id.replace(/^[a-z]+/u, '')) > minted) reject()
  }
  for (const { id, host } of pending) {
    const entry = layout.nodes[id]
    if (visited.has(id) || entry === undefined) reject()
    visited.add(id)
    if (entry.kind === 'split') {
      if (host !== 'dock' || id !== layout.rootId || entry.axis !== 'row' || entry.children.length !== 2
        || entry.children.length !== entry.sizes.length
        || Math.abs(entry.sizes.reduce((sum, size) => sum + size, 0) - 1) > 1e-9) reject()
      pending.push(...entry.children.map(id => ({ id, host })))
    } else {
      if (entry.host !== host || (host === 'float' ? entry.rect === undefined || entry.tabs.length !== 1 : entry.rect !== undefined)) reject()
      if (entry.tabs.length === 0 ? entry.activeTabId !== undefined : !entry.tabs.includes(entry.activeTabId!)) reject()
      for (const tab of entry.tabs) {
        if (usedTabs.has(tab) || layout.tabs[tab] === undefined) reject()
        usedTabs.add(tab)
      }
    }
  }
  if (visited.size !== Object.keys(layout.nodes).length || usedTabs.size !== Object.keys(layout.tabs).length
    || layout.nodes[layout.activePaneId]?.kind !== 'pane') reject()
}

/**
 * Restore one validated Session layout with a fresh in-window undo history.
 * @param sessionId - storage scope.
 * @returns the saved surface, or undefined when absent, inaccessible or invalid.
 */
export function readSidebarLayout(sessionId: string): SurfaceState | undefined {
  if (typeof localStorage === 'undefined') return undefined
  let raw: string | null
  try { raw = localStorage.getItem(`${sidebarPersistence}.${sessionId}`) }
  catch (_storageUnavailable) { return undefined }
  if (raw === null) return undefined
  try {
    const envelope: unknown = JSON.parse(raw)
    if (!record(envelope) || !record(envelope.bySession)) throw new Error('Invalid sidebar envelope')
    const saved = envelope.bySession[sessionId]
    if (saved === undefined) return undefined
    const parsed = parseSurface(saved)
    validateReferences(parsed)
    return { layout: parsed.layout as unknown as LayoutState, minted: parsed.minted, history: EMPTY_HISTORY }
  } catch (_invalidLayout) {
    clearSidebarLayout(sessionId)
    return undefined
  }
}

/**
 * Persist current layout and identity allocation without retaining undo entries.
 * @param sessionId - storage scope.
 * @param surface - current in-memory surface.
 */
export function writeSidebarLayout(sessionId: string, surface: SurfaceState): void {
  if (typeof localStorage === 'undefined') return
  const saved = { bySession: { [sessionId]: { layout: surface.layout, minted: surface.minted } } }
  try { localStorage.setItem(`${sidebarPersistence}.${sessionId}`, JSON.stringify(saved)) }
  catch (error) { console.error('Sidebar layout persistence failed:', error) }
}

/**
 * Remove only one Session's persisted layout.
 * @param sessionId - storage scope to discard.
 */
export function clearSidebarLayout(sessionId: string): void {
  if (typeof localStorage === 'undefined') return
  try { localStorage.removeItem(`${sidebarPersistence}.${sessionId}`) }
  catch (_storageUnavailable) { /* The invalid layout is still excluded from this window. */ }
}
