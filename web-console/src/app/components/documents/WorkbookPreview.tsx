import { useState } from 'react';
import { useTranslation } from '../../../locale/react';
import type { WorkbookPreview as Workbook } from '../../../../shared/documents.ts';

/** A bounded window mounts at most 100 rows, independent of admitted cell count. */
export function WorkbookPreview({ workbook }: { workbook: Workbook }) {
  const tx = useTranslation(), [sheetIndex, setSheet] = useState(0), [offset, setOffset] = useState(0);
  const sheet = workbook.sheets[sheetIndex];
  return <div className="document-view"><div className="document-toolbar">
    <label>{tx('artifacts:document.sheet')}<select aria-label={tx('artifacts:document.sheet')} value={sheetIndex} onChange={event => { setSheet(+event.target.value); setOffset(0); }}>
      {workbook.sheets.map((sheet, index) => <option key={index} value={index}>{sheet.name}</option>)}
    </select></label>
    <button disabled={!offset} onClick={() => setOffset(offset - 100)}>{tx('artifacts:document.previous')}</button>
    <button disabled={offset + 100 >= sheet.cells.length} onClick={() => setOffset(offset + 100)}>{tx('artifacts:document.next')}</button>
  </div>{sheet.truncated && <p role="status">{tx('artifacts:document.truncated')}</p>}
    <table><thead><tr><th>{tx('artifacts:document.cell')}</th><th>{tx('artifacts:document.value')}</th><th>{tx('artifacts:document.formula')}</th></tr></thead>
      <tbody>{sheet.cells.slice(offset, offset + 100).map(cell => <tr key={cell.address}><th scope="row">{cell.address}</th>
        <td>{cell.value ?? tx('artifacts:document.no_value')}</td><td>{cell.formula}</td></tr>)}</tbody>
    </table></div>;
}
