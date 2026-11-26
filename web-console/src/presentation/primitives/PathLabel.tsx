/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
/** A single-line file path whose trailing characters remain visible in narrow toolbars. */
import { useLayoutEffect, useRef, type HTMLAttributes } from 'react'
import clsx from 'clsx'
const pathPartsOf = (path: string) => { const i = path.lastIndexOf('/'); return { directory: path.slice(0, i + 1), name: path.slice(i + 1) }; }

/**
 * Render subdued directories and a primary filename, with the complete path on hover.
 * Fitting text is left-aligned; overflow clips and fades at the left edge.
 * The fade updates on path changes and, when ResizeObserver is available, size changes.
 * @param props - File path and attributes for its outer span; callers own toolbar spacing.
 * @returns the path label.
 */
export function PathLabel({ path, className, ...attributes }: {
  path: string
} & Omit<HTMLAttributes<HTMLSpanElement>, 'children' | 'title'>) {
  const boxRef = useRef<HTMLSpanElement>(null)
  const textRef = useRef<HTMLSpanElement>(null)
  const { directory, name } = pathPartsOf(path)
  useLayoutEffect(() => {
    // Both spans mount unconditionally before this layout effect runs.
    const outer = boxRef.current as HTMLSpanElement
    const inner = textRef.current as HTMLSpanElement
    const apply = (): void => {
      outer.toggleAttribute('data-path-clipped', inner.offsetWidth > outer.clientWidth)
    }
    apply()
    const observer = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(apply)
    observer?.observe(outer)
    observer?.observe(inner)
    return () => { observer?.disconnect() }
  }, [path])
  return (
    <span {...attributes} ref={boxRef} className={clsx('flex flex-1 justify-end min-w-0 overflow-hidden text-[12px] whitespace-nowrap [&[data-path-clipped]]:[mask-image:linear-gradient(to_right,transparent,black_28px)]', className)} title={path} data-path-label>
      <span ref={textRef} className="flex-none mr-auto">
        {directory !== '' && <span className="[color:var(--dsw-alias-label-tertiary)]">{directory}</span>}
        <span className="[color:var(--dsw-alias-label-primary)]">{name}</span>
      </span>
    </span>
  )
}
