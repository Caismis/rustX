import { lazy, Suspense } from 'react';
import { useLocale, useTranslation } from '../../../locale/react';
import { en, zh } from '../../../locale/dictionaries/workbench-excel';
import { FILE_PREVIEW_LIMITS } from '../../../../shared/file-preview.ts';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';

const Excel = lazy(() => import('../workbench-documents/excel/excel').then(module => ({ default: module.ExcelBody })));
const limits = { maxBytes: FILE_PREVIEW_LIMITS.spreadsheetBytes, maxCells: FILE_PREVIEW_LIMITS.spreadsheetCells, timeoutMs: FILE_PREVIEW_LIMITS.parserTimeoutMs };

/** Conversation and workspace files use the same bounded browser parser/viewer. */
export function SpreadsheetPreview({ bytes, signal, viewState, onViewStateChange }: PreviewViewStateProps & { bytes: Uint8Array<ArrayBuffer>; signal: AbortSignal }) {
  const locale = useLocale(), tx = useTranslation(), dictionary = locale.active === 'zh' ? zh : en;
  const loading = <p role="status">{tx('artifacts:document.loading')}</p>;
  return <Suspense fallback={loading}><Excel content={{ kind: 'bytes', data: bytes }} format="xlsx" limits={limits} loading={loading}
    signal={signal} activeSheet={viewState.workbookSheet} onActiveSheetChange={workbookSheet => onViewStateChange({ workbookSheet })}
    t={(key, parameters) => Object.entries(parameters ?? {}).reduce((value, [name, replacement]) => value.replaceAll(`{${name}}`, String(replacement)), dictionary[key])}/></Suspense>;
}
