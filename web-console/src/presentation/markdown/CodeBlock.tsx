/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
import { Fragment, useCallback, useMemo, useRef, useState, useSyncExternalStore } from 'react'
import type { CSSProperties, ReactNode, Ref } from 'react'
import clsx from 'clsx'
import { CodeToolbar } from './CodeToolbar';
import { writeClipboard } from '../primitives/clipboard.ts'
import {
  StreamingHighlightSession, grammarLoadCount, highlightToCodeHtml, subscribeGrammarLoaded,
} from './highlight.ts'
import type { HighlightSpan, StreamingHighlightFrame } from './highlight.ts'
import { useViewportHighlighting } from './useViewportHighlighting.ts'

export interface CodeBlockProps {
  /** The source text, rendered verbatim (trailing newline trimmed for display). */
  code: string
  /** Grammar hint (markdown fence info string or a fixed caller id); unknown = plain. */
  lang?: string | undefined
  /**
   * The code is still growing (a streaming markdown fence): highlight through
   * a per-instance {@link StreamingHighlightSession}, which re-tokenizes only
   * appended text and keeps completed line groups (and DOM) untouched. The
   * caller must keep the component instance stable across growth (a
   * stream-stable React key). Settlement discards these transient caches and
   * always uses the same renderer as a cold settled mount.
   */
  streaming?: boolean | undefined
  /** Extra class merged onto the wrapper (callers position; this component draws). */
  className?: string | undefined
  /** Ref for the stable source-content wrapper, for owners that use it as a scrollport. */
  contentRef?: Ref<HTMLDivElement> | undefined
  /** Show a numbered gutter without adding numbers to copied source. Defaults to false. */
  lineNumbers?: boolean | undefined
  /** Copy-button idle label; the owner passes localized copy (this package is cordis-free, so copy arrives via props). */
  /** Use the document viewer toolbar; the owner supplies its localized language fallback. */
  toolbarLabel?: string
  copyLabel: string
  /** Copy-button label during the post-copy confirmation window. */
  copiedLabel: string
}

/**
 * The `pre` attributes shiki's HTML arm emits for the css-variables theme,
 * mirrored so the streaming arm's tree is interchangeable with the settled
 * swap (`tests/streaming-code-block.client.spec.tsx` pins the two arms'
 * parity).
 */
const SHIKI_PRE_PROPS = {
  className: 'shiki css-variables',
  style: { backgroundColor: 'var(--shiki-background)', color: 'var(--shiki-foreground)' },
  tabIndex: 0,
} as const

/** Completed-line group size; React reconciles groups while the DOM remains line-for-line identical. */
const STREAMING_LINE_GROUP_SIZE = 32

function renderLine(line: readonly HighlightSpan[], index: number): ReactNode {
  return (
    <Fragment key={index}>
      {index > 0 && '\n'}
      <span className="line">
        {line.map((span, spanIndex) => <span key={spanIndex} style={span.style}>{span.text}</span>)}
      </span>
    </Fragment>
  )
}

export function CodeBlock({ code, lang, streaming, className, contentRef, lineNumbers = false, copyLabel, copiedLabel, toolbarLabel }: CodeBlockProps) {
  const trimmed = code.endsWith('\n') ? code.slice(0, -1) : code
  const sourceLines = lineNumbers ? trimmed.split('\n') : undefined
  const rootRef = useRef<HTMLDivElement>(null)
  const highlighting = useViewportHighlighting(rootRef, lang)
  // Re-render when a lazy grammar finishes loading, so a fence that showed plain
  // text while its language's grammar imported picks up highlighting. The
  // snapshot value is opaque; only its change across renders drives the memo.
  const loaded = useSyncExternalStore(subscribeGrammarLoaded, grammarLoadCount, grammarLoadCount)
  // Streaming state lives in refs mutated inside the memo (the MarkdownText
  // streaming-cache pattern): the session's caches carry across chunks only
  // because the owner keys this instance stably while the fence grows.
  const sessionRef = useRef<StreamingHighlightSession | null>(null)
  const lineCacheRef = useRef<{
    code: string
    lang: string | undefined
    generation: number
    frame: StreamingHighlightFrame
    groups: ReactNode[]
    pending: ReactNode[]
    nextLine: number
    body: ReactNode
  } | null>(null)
  const streamedBody = useMemo(() => {
    // Settlement is a boundary, not another cache mode. Never retain a
    // streaming tree based on whether a lazy grammar happened to load in time.
    if (streaming !== true || !highlighting) {
      sessionRef.current = null
      lineCacheRef.current = null
      return undefined
    }
    sessionRef.current ??= new StreamingHighlightSession()
    const frame = sessionRef.current.updateFrame(trimmed, lang)
    if (frame === undefined) {
      lineCacheRef.current = null
      return undefined
    }
    const previous = lineCacheRef.current
    if (previous?.frame === frame && previous.code === trimmed && previous.lang === lang) {
      return previous.body
    }
    const sameGeneration = previous?.generation === frame.generation
    const groups = sameGeneration ? [...previous.groups] : []
    let pending = sameGeneration ? [...previous.pending] : []
    let nextLine = sameGeneration ? previous.nextLine : 0
    for (const line of frame.appended) {
      pending.push(renderLine(line, nextLine))
      nextLine += 1
      if (pending.length !== STREAMING_LINE_GROUP_SIZE) continue
      const start = nextLine - pending.length
      groups.push(<Fragment key={start}>{pending}</Fragment>)
      pending = []
    }
    const tail = frame.tail.map((line, index) => renderLine(line, nextLine + index))
    const tailGroup = <Fragment key={nextLine - pending.length}>{[...pending, ...tail]}</Fragment>
    const body = <pre {...SHIKI_PRE_PROPS}><code>{groups}{tailGroup}</code></pre>
    lineCacheRef.current = {
      code: trimmed, lang, generation: frame.generation, frame, groups, pending, nextLine, body,
    }
    return body
  }, [streaming, highlighting, trimmed, lang, loaded])
  const html = useMemo(
    () => (highlighting && streaming !== true
      ? highlightToCodeHtml(trimmed, lang)
      : undefined),
    [streaming, highlighting, trimmed, lang, loaded],
  )
  const [copied, setCopied] = useState(false)

  const onCopy = useCallback(() => {
    if (copied) return
    /* v8 ignore next -- both arms always mount a <pre>; trimmed is the
       typed fallback if the DOM shape ever diverges. */
    const text = rootRef.current?.querySelector('pre')?.textContent ?? trimmed
    void writeClipboard(text).then((ok) => {
      if (!ok) return
      setCopied(true)
      window.setTimeout(() => { setCopied(false) }, 1000)
    })
  }, [copied, trimmed])

  // shiki's HTML output is a static span tree it generated from `code` (no
  // user HTML passes through), the sanctioned innerHTML consumption path per
  // shiki's own docs.
  const body = streamedBody !== undefined
    ? streamedBody
    : html === undefined
      ? (
        <pre className="[color:var(--dsw-alias-label-primary)]"><code>{sourceLines === undefined ? trimmed : sourceLines.map((line, index) => (
          <Fragment key={index}>{index > 0 && '\n'}<span className="line">{line}</span></Fragment>
        ))}</code></pre>
      )
      : (
        <pre {...SHIKI_PRE_PROPS}><code dangerouslySetInnerHTML={{ __html: html }}/></pre>
      )

  return (
    <div ref={rootRef} className={clsx('relative mx-0 my-[16px] [color:var(--dsw-alias-label-primary)] bg-[var(--dsl-code-block-background)] rounded-[12px] [--dsl-code-block-banner-background-color:var(--dsw-alias-markdown-code-block-banner)] [--dsl-code-block-border-radius:12px] [--dsl-code-block-banner-font:11px/18px_var(--dsw-font-family)] [--dsl-code-block-content-font:var(--dsw-font-markdown-code-block)] [--dsl-code-block-background:var(--dsw-alias-markdown-code-block)] [&:not(:last-child)]:mb-[11px] [&_pre]:[font:var(--dsl-code-block-content-font)] [&_pre]:p-[16px] [&_pre]:m-0! [&_pre]:overflow-x-auto [&_pre]:whitespace-pre-wrap [&_pre]:break-all [&_pre]:bg-[var(--dsl-code-block-background)] [&_pre]:rounded-b-[12px] [&_pre.shiki]:bg-[var(--dsl-code-block-background)]! [&_pre_code]:[font:inherit] [&_pre_code]:bg-none [&_pre_code]:p-0', 'md-code-block', lineNumbers && '[&_pre_code]:block [&_pre_code]:[counter-reset:source-line] [&_pre_code]:whitespace-normal [&_pre_code_.line]:relative [&_pre_code_.line]:block [&_pre_code_.line]:min-h-[1lh] [&_pre_code_.line]:ps-[calc(var(--dsl-code-block-line-number-width)+12px)] [&_pre_code_.line]:[counter-increment:source-line] [&_pre_code_.line]:whitespace-[var(--dsl-code-block-line-white-space,pre-wrap)] [&_pre_code_.line]:before:absolute [&_pre_code_.line]:before:start-0 [&_pre_code_.line]:before:w-[var(--dsl-code-block-line-number-width)] [&_pre_code_.line]:before:[color:var(--dsw-alias-label-tertiary)] [&_pre_code_.line]:before:text-end [&_pre_code_.line]:before:content-[counter(source-line)] [&_pre_code_.line]:before:select-none', className)}
      data-line-numbers={lineNumbers || undefined}
      style={sourceLines === undefined ? undefined : {
        '--dsl-code-block-line-number-width': `${Math.max(2, String(sourceLines.length).length)}ch`,
      } as CSSProperties}>
      {/* These paired attributes are stable semantic hooks for owner styling and DOM tests. */}
      <div className="sticky top-0 z-[6] bg-[var(--dsw-alias-bg-base)] rounded-t-[12px]">
        <>{toolbarLabel !== undefined ? <CodeToolbar lang={lang} label={toolbarLabel} copied={copied} copyLabel={copyLabel} copiedLabel={copiedLabel} onCopy={onCopy}/> : <div className="bg-[var(--dsl-code-block-banner-background-color)] px-[14px] py-[9px] flex justify-between items-center gap-[12px] [font:var(--dsl-code-block-banner-font)] rounded-t-[12px]" data-code-block-banner>
          <div className="[color:var(--dsw-alias-label-primary)] [font-family:var(--ds-font-family-code),monospace] text-[11px] leading-[18px] min-w-0 overflow-hidden text-ellipsis whitespace-nowrap">{lang ?? ''}</div>
          <div className="flex items-center shrink-0">
            <button type="button" className="bg-transparent border-none p-0 m-0 [color:inherit] cursor-pointer [font:inherit]" onClick={onCopy} aria-label={copied ? copiedLabel : copyLabel} title={copied ? copiedLabel : copyLabel}>
              {copied ? copiedLabel : copyLabel}
            </button>
          </div>
        </div>}</>
      </div>
      <div ref={contentRef} className="contents" data-code-block-content>{body}</div>
    </div>
  )
}
