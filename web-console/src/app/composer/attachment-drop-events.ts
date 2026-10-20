/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
/** Document drag-and-drop listeners owned by one mounted attachment view. */
import { transferInputs, type IntakeInput } from '../../client/uploads'

/**
 * Install one attachment view's file-drop listeners.
 * @param canAcceptDrop - whether this view accepts the dropped files.
 * @param onAddFiles - attachment intake callback.
 * @param dragDepth - the view's retained nested-drag counter.
 * @param setDragActive - publish whether a file drag is active.
 * @param owner - the retained composer's DOM owner, including its visibility.
 * @returns cleanup for exactly these listeners.
 */
export function installDocumentDropEvents(
  canAcceptDrop: boolean,
  onAddFiles: (inputs: IntakeInput[]) => void,
  dragDepth: { current: number },
  setDragActive: (active: boolean) => void,
  owner: { readonly current: HTMLElement | null },
): () => void {
  const visible = (node: Element | null): boolean => !!node && !node.closest('[hidden], [inert]')
  const owns = (event: globalThis.DragEvent): boolean => {
    const element = owner.current
    if (!element?.isConnected || !visible(element) || event.defaultPrevented) return false
    const target = event.target instanceof Element ? event.target.closest('[data-file-drop-owner]') : null
    if (target) return target === element
    const focused = document.activeElement?.closest('[data-file-drop-owner]') ?? null
    // A targeted or focused pane wins. Otherwise the first visible listener
    // claims the document drop by preventing its default, exactly once.
    return !visible(focused) || focused === element
  }
  const fileTransfer = (event: globalThis.DragEvent): DataTransfer | null => {
    const dataTransfer = event.dataTransfer
    if (dataTransfer === null || !dataTransfer.types.includes('Files')) return null
    return dataTransfer
  }
  const reset = (): void => {
    dragDepth.current = 0
    setDragActive(false)
  }
  const onDragEnter = (event: globalThis.DragEvent): void => {
    if (fileTransfer(event) === null) return
    if (!owns(event)) { reset(); return }
    event.preventDefault()
    dragDepth.current += 1
    setDragActive(true)
  }
  const onDragOver = (event: globalThis.DragEvent): void => {
    const dataTransfer = fileTransfer(event)
    if (dataTransfer === null) return
    if (!owns(event)) { reset(); return }
    event.preventDefault()
    dataTransfer.dropEffect = canAcceptDrop ? 'copy' : 'none'
  }
  const onDragLeave = (event: globalThis.DragEvent): void => {
    if (fileTransfer(event) === null) return
    dragDepth.current = Math.max(0, dragDepth.current - 1)
    if (dragDepth.current === 0) setDragActive(false)
    const leftViewport = event.clientX <= 0 || event.clientY <= 0
      || event.clientX >= window.innerWidth || event.clientY >= window.innerHeight
    if ((event.target === document.documentElement || event.target === document.body) && leftViewport) reset()
  }
  const onDrop = (event: globalThis.DragEvent): void => {
    const dataTransfer = fileTransfer(event)
    if (dataTransfer === null) return
    reset()
    if (!owns(event)) return
    event.preventDefault()
    if (canAcceptDrop) {
      onAddFiles(transferInputs(dataTransfer))
    }
  }
  document.addEventListener('dragenter', onDragEnter)
  document.addEventListener('dragover', onDragOver)
  document.addEventListener('dragleave', onDragLeave)
  document.addEventListener('drop', onDrop)
  window.addEventListener('dragend', reset)
  return () => {
    document.removeEventListener('dragenter', onDragEnter)
    document.removeEventListener('dragover', onDragOver)
    document.removeEventListener('dragleave', onDragLeave)
    document.removeEventListener('drop', onDrop)
    window.removeEventListener('dragend', reset)
  }
}
