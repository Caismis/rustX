/* Copyright (c) 2026 DeepSeek. MIT. Source port; see PROVENANCE.md. */
/** Read-only spreadsheet surface backed by browser-parsed workbook data. */
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { Workbook } from '@fortune-sheet/react'
import { Button, IconWarningTriangleOutlineRegular, Tooltip } from '../primitives'
import fortuneCss from '@fortune-sheet/react/dist/index.css?inline'
import type { ExcelFormat } from './format.ts'
import type { ExcelPreview } from './model.ts'
import type { ExcelLimits } from './model';
import type { en } from '../../../../locale/dictionaries/workbench-excel';
type LoadedExcelBodyProps = {content: {kind:'bytes';data:Uint8Array<ArrayBuffer>};format:ExcelFormat;limits:ExcelLimits;loading:ReactNode;t:(key:keyof typeof en,params?:Record<string,string|number>)=>string; signal?:AbortSignal; activeSheet?:number; onActiveSheetChange?:(index:number)=>void};
import { parseExcel } from './parse.ts'

type State = { data: Uint8Array<ArrayBuffer>; format: ExcelFormat } & ({ value: ExcelPreview } | { error: string })
const scopedStyles = `@scope ([data-excel-preview]) { ${fortuneCss} }`

/**
 * Display stored spreadsheet values and formats without editing or recalculation.
 * Keep the mounted workbook sized to its preview pane.
 * @param props - Complete workbook bytes, limits, and locale.
 * @returns An isolated spreadsheet surface with cancellable loading.
 */
export function ExcelBody({ content, format, limits, t, loading, signal, activeSheet, onActiveSheetChange }: LoadedExcelBodyProps): ReactNode {
  const data = content.kind === 'bytes' ? content.data : undefined
  const [state, setState] = useState<State | undefined>(undefined)
  const [attempt, setAttempt] = useState(0)
  const workbookRef = useRef<HTMLDivElement>(null)
  const retainedSheet = useRef(activeSheet)
  retainedSheet.current = activeSheet
  useEffect(() => {
    if (data === undefined) return
    const controller = new AbortController()
    const lifetime = signal ? AbortSignal.any([signal, controller.signal]) : controller.signal
    if (lifetime.aborted) return () => controller.abort()
    setState(undefined)
    void parseExcel(data, format, limits, lifetime).then(
      (value) => {
        if (lifetime.aborted) return
        if (retainedSheet.current !== undefined) {
          let index = Math.max(0, Math.min(retainedSheet.current, value.sheets.length - 1))
          if (value.sheets[index].hide) index = Math.max(0, value.sheets.findIndex(sheet => !sheet.hide))
          value = { ...value, sheets: value.sheets.map((sheet, at) => ({ ...sheet, status: at === index ? 1 : 0 })) }
        }
        setState({ data, format, value })
      },
      (error: unknown) => { if (!lifetime.aborted) setState({ data, format, error: error instanceof Error ? error.message : 'invalid' }) },
    )
    return () => { controller.abort() }
  }, [data, format, limits, attempt, signal])
  useEffect(() => {
    const element = workbookRef.current
    if (element === null || typeof ResizeObserver === 'undefined') return
    // FortuneSheet measures its canvas on window resize, but pane resizing does not emit that event.
    const observer = new ResizeObserver(() => { window.dispatchEvent(new Event('resize')) })
    observer.observe(element)
    return () => { observer.disconnect() }
  }, [state])
  if (data === undefined) return <p className="flex gap-[8px] items-center justify-center h-full p-[16px] [color:var(--dsw-alias-label-secondary)]" role="alert">{t('invalid')}</p>
  if (state?.data !== data || state.format !== format) return loading
  if ('error' in state) return <div className="flex gap-[8px] items-center justify-center h-full p-[16px] [color:var(--dsw-alias-label-secondary)]" role="alert">
    <span>{t(state.error === 'tooLarge' || state.error === 'timeout' || state.error === 'encoding' ? state.error : 'invalid')}</span>
    <Button size="sm" onClick={() => { setAttempt(value => value + 1) }}>{t('retry')}</Button>
  </div>
  const hasFormulas = state.value.sheets.some(sheet => sheet.celldata?.some(cell => cell.v?.f !== undefined))
  return <section className="flex flex-col h-full min-h-0 overflow-hidden whitespace-normal [font-family:var(--dsw-font,sans-serif)]" data-excel-preview aria-label={t('title')}>
    <style>{scopedStyles}</style>
    {state.value.unsupportedFeatures.length > 0 && <div className="flex flex-none items-start gap-[8px] px-[12px] py-[8px] [color:var(--dsw-alias-state-warn-label)] bg-[var(--dsw-alias-state-warn-tertiary)] [font-size:var(--dsh-content-font-size-secondary,13px)] leading-[1.5] wrap-anywhere [&>svg]:flex-none [&>svg]:mt-[2px]" role="note" data-excel-unsupported-notice>
      <IconWarningTriangleOutlineRegular size={16} />
      <span>{t('unsupportedNotice', { features: state.value.unsupportedFeatures.map(feature => t(feature)).join(t('featureSeparator')) })}</span>
    </div>}
    <div ref={workbookRef} className={`relative flex-1 min-h-0 min-w-0 [color-scheme:light] [color:CanvasText] [&_.fortune-stat-area]:hidden [&_.luckysheet-sheets-item-function]:hidden [&_.fortune-cols-freeze-handle]:hidden [&_.fortune-rows-freeze-handle]:hidden [&_.luckysheet-sheet-area_div.luckysheet-sheets-item]:pr-[5px] [&_.luckysheet-sheet-area]:p-0 [&_#luckysheet-sheet-content]:flex-1 [&_#luckysheet-sheet-content]:min-w-0 [&_.fortune-sheettab-container]:flex-1 [&_.fortune-sheettab-container]:min-w-0 [&_.fortune-sheettab-container]:max-w-full [&_.fortune-sheettab-container_.fortune-sheettab-container-c]:overflow-x-auto [&_.fortune-sheettab-container_.fortune-sheettab-container-c]:overflow-y-hidden [&_.fortune-sheettab-container_.fortune-sheettab-container-c]:overscroll-x-contain [&_.fortune-sheettab-container_.fortune-sheettab-container-c]:[scrollbar-width:none] [&_.fortune-sheettab-scroll]:flex-none [&_.fortune-sheet-area-right]:flex-none [&_.fortune-fx-input-container]:min-w-0 [&_.fortune-fx-input]:min-w-0 [&_.fortune-fx-input]:overflow-x-auto [&_.fortune-fx-input]:overflow-y-hidden [&_.fortune-fx-input]:whitespace-nowrap [&_.fortune-fx-input]:[scrollbar-width:none] ${hasFormulas ? '[&_.fortune-fx-input]:pl-[24px]' : ''}`}>
      {hasFormulas && <Tooltip label={t('formulaWarning')} side="bottom" delayMs={500}>
        <button type="button" className="absolute top-[4px] left-[142px] z-[3] flex w-[20px] h-[20px] p-[3px] [color:var(--dsw-alias-state-warn-label)] bg-transparent border-0 rounded-[var(--dsw-radius-sm)] focus-visible:bg-[var(--dsw-alias-state-warn-tertiary)] focus-visible:outline-none hover:bg-[var(--dsw-alias-state-warn-tertiary)]" aria-label={t('formulaWarning')} data-excel-formula-warning>
          <IconWarningTriangleOutlineRegular size={14} />
        </button>
      </Tooltip>}
      <Workbook key={`${attempt}:${t('language')}`} data={state.value.sheets} lang={t('language')}
        hooks={{ afterActivateSheet: id => { const index = state.value.sheets.findIndex(sheet => sheet.id === id); if (index >= 0 && !signal?.aborted) onActiveSheetChange?.(index) } }}
        allowEdit={false} showToolbar={false} showFormulaBar showSheetTabs forceCalculation={false}
        cellContextMenu={['copy']} headerContextMenu={[]} sheetTabContextMenu={[]} filterContextMenu={[]} />
    </div>
  </section>
}
