import { useTranslation } from '../../../locale/react';
import type { WorkbookPreview as Workbook } from '../../../../shared/documents.ts';
import type { PreviewViewStateProps } from '../../../presentation/right-panel/preview-view-state';
import { useViewScroll } from '../../../presentation/right-panel/use-view-scroll';

/** A bounded window mounts at most 100 rows, independent of admitted cell count. */
export function WorkbookPreview({ workbook, viewState, onViewStateChange }: PreviewViewStateProps & { workbook: Workbook }) {
  const tx = useTranslation(), sheetIndex = Math.max(0, Math.min(viewState.workbookSheet ?? 0, workbook.sheets.length - 1));
  const sheet = workbook.sheets[sheetIndex];
  const offset = Math.max(0, Math.min(viewState.workbookOffset ?? 0, Math.floor(Math.max(0, (sheet?.cells.length ?? 0) - 1) / 100) * 100));
  const scroll = useViewScroll(viewState.workbookScrollTop ?? 0, viewState.workbookScrollLeft ?? 0,
    (workbookScrollTop, workbookScrollLeft) => onViewStateChange({ workbookScrollTop, workbookScrollLeft }), `${sheetIndex}:${offset}`);
  const windowAt = (workbookOffset: number) => onViewStateChange({ workbookOffset, workbookScrollTop: 0, workbookScrollLeft: 0 });
  return <div className="document-view" ref={scroll.ref} onScroll={scroll.onScroll} data-preview-scroll="workbook"><div className="document-toolbar">
    <label>{tx('artifacts:document.sheet')}<select aria-label={tx('artifacts:document.sheet')} value={sheetIndex} onChange={event => onViewStateChange({ workbookSheet: +event.target.value, workbookOffset: 0, workbookScrollTop: 0, workbookScrollLeft: 0 })}>
      {workbook.sheets.map((sheet, index) => <option key={index} value={index}>{sheet.name}</option>)}
    </select></label>
    <button disabled={!offset} onClick={() => windowAt(offset - 100)}>{tx('artifacts:document.previous')}</button>
    <button disabled={offset + 100 >= (sheet?.cells.length ?? 0)} onClick={() => windowAt(offset + 100)}>{tx('artifacts:document.next')}</button>
  </div>{sheet?.truncated && <p role="status">{tx('artifacts:document.truncated')}</p>}
    <table><thead><tr><th>{tx('artifacts:document.cell')}</th><th>{tx('artifacts:document.value')}</th><th>{tx('artifacts:document.formula')}</th></tr></thead>
      <tbody>{sheet?.cells.slice(offset, offset + 100).map(cell => <tr key={cell.address}><th scope="row">{cell.address}</th>
        <td>{cell.value ?? tx('artifacts:document.no_value')}</td><td>{cell.formula}</td></tr>)}</tbody>
    </table></div>;
}
