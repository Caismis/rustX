// @vitest-environment node
import ExcelJS from 'exceljs';
import { unzipSync, zipSync } from 'fflate';
import { expect, it, vi } from 'vitest';
import { convertExcel } from '../src/app/components/workbench-documents/excel/convert';

async function fixture(sheets: string[]) {
  const workbook = new ExcelJS.Workbook();
  sheets.forEach((_, index) => workbook.addWorksheet(`Sheet ${index + 1}`));
  const files = unzipSync(new Uint8Array(await workbook.xlsx.writeBuffer()));
  sheets.forEach((xml, index) => {
    files[`xl/worksheets/sheet${index + 1}.xml`] = new TextEncoder().encode(
      `<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">${xml}</worksheet>`);
  });
  return files;
}
const limits = (maxCells: number) => ({ maxBytes: 16 * 1024 * 1024, maxCells, timeoutMs: 15000 });

it.each(['5000000', '4294967294', '&#53;000000'])('rejects allocation-driving workbook sheetId %s before the ExcelJS loader', async id => {
  const files = await fixture(['<sheetData/><mergeCells><mergeCell ref="A1:B2"/></mergeCells>']);
  files['xl/workbook.xml'] = new TextEncoder().encode(new TextDecoder().decode(files['xl/workbook.xml']).replace(/sheetId="[^"]+"/, `sheetId="${id}"`));
  // The loader accessor itself is intercepted: even a broken regression cannot
  // allocate a huge sparse worksheet array in this test process.
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get').mockImplementation(() => { throw new Error('ExcelJS loader reached'); });
  await expect(convertExcel(zipSync(files, { level: 0 }), 'xlsx', limits(4))).rejects.toMatchObject({ code: 'tooLarge' });
  expect(load).not.toHaveBeenCalled();
});
it.each(['0', '-1', '01', '1e6', '1garbage', '1.5', ''])('rejects ambiguous workbook sheetId %s before loading', async id => {
  const files = await fixture(['<sheetData/>']);
  files['xl/workbook.xml'] = new TextEncoder().encode(new TextDecoder().decode(files['xl/workbook.xml']).replace(/sheetId="[^"]+"/, `sheetId="${id}"`));
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get').mockImplementation(() => { throw new Error('ExcelJS loader reached'); });
  await expect(convertExcel(zipSync(files, { level: 0 }), 'xlsx', limits(4))).rejects.toMatchObject({ code: 'invalid' });
  expect(load).not.toHaveBeenCalled();
});
it('rejects duplicate workbook worksheet indices before loading', async () => {
  const files = await fixture(['<sheetData/>', '<sheetData/>']);
  files['xl/workbook.xml'] = new TextEncoder().encode(new TextDecoder().decode(files['xl/workbook.xml']).replace(/sheetId="2"/, 'sheetId="1"'));
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get').mockImplementation(() => { throw new Error('ExcelJS loader reached'); });
  await expect(convertExcel(zipSync(files, { level: 0 }), 'xlsx', limits(4))).rejects.toMatchObject({ code: 'invalid' }); expect(load).not.toHaveBeenCalled();
});

async function namedReference(value: string) {
  const files = await fixture(['<sheetData/>']);
  const book = new TextDecoder().decode(files['xl/workbook.xml']);
  files['xl/workbook.xml'] = new TextEncoder().encode(book.replace('</workbook>', `<definedNames><definedName name="Values">${value}</definedName></definedNames></workbook>`));
  return zipSync(files);
}

it.each([
  ['merge rectangle', '<sheetData/><mergeCells count="1"><mergeCell ref="A1:C2"/></mergeCells>'],
  ['sparse cell', '<sheetData><row r="1"><c r="E1"><v>1</v></c></row></sheetData>'],
  ['empty row', '<sheetData><row r="5"/></sheetData>'],
  ['column range', '<cols><col min="1" max="5" width="10"/></cols><sheetData/>'],
  ['validation range', '<sheetData/><dataValidations count="1"><dataValidation type="whole" sqref="A1:C2"><formula1>1</formula1></dataValidation></dataValidations>'],
  ['repeated cells', '<sheetData><row r="1">' + '<c r="A1"><v>1</v></c>'.repeat(5) + '</row></sheetData>'],
])('rejects an over-budget %s before ExcelJS loading', async (_name, xml) => {
  const bytes = zipSync(await fixture([xml]));
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get');
  await expect(convertExcel(bytes, 'xlsx', limits(4))).rejects.toMatchObject({ code: 'tooLarge' });
  expect(load).not.toHaveBeenCalled();
});

it('checks unreferenced worksheet entries recognized by ExcelJS before loading', async () => {
  const files = await fixture(['<sheetData/>']);
  files['extra/xl/worksheets/sheet2.xml.backup'] = new TextEncoder().encode('<worksheet><sheetData/><mergeCells><mergeCell ref="A1:C2"/></mergeCells></worksheet>');
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get');
  await expect(convertExcel(zipSync(files), 'xlsx', limits(4))).rejects.toMatchObject({ code: 'tooLarge' });
  expect(load).not.toHaveBeenCalled();
});

it.each(['A0', 'XFE1', 'A1048577', 'A1:B2', 'A1garbage'])('rejects invalid cell coordinate %s before loading', async ref => {
  const bytes = zipSync(await fixture([`<sheetData><row r="1"><c r="${ref}"><v>1</v></c></row></sheetData>`]));
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get');
  await expect(convertExcel(bytes, 'xlsx', limits(4))).rejects.toMatchObject({ code: 'invalid' });
  expect(load).not.toHaveBeenCalled();
});

it('checks aggregate area before loading either worksheet', async () => {
  const xml = '<sheetData/><mergeCells count="1"><mergeCell ref="A1:B2"/></mergeCells>';
  const bytes = zipSync(await fixture([xml, xml]));
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get');
  await expect(convertExcel(bytes, 'xlsx', limits(7))).rejects.toMatchObject({ code: 'tooLarge' });
  expect(load).not.toHaveBeenCalled();
});

it('does not trust a small dimension declaration or the merge count', async () => {
  const bytes = zipSync(await fixture(['<dimension ref="A1"/><sheetData/><mergeCells count="0"><mergeCell ref="A1:C2"/></mergeCells>']));
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get');
  await expect(convertExcel(bytes, 'xlsx', limits(4))).rejects.toMatchObject({ code: 'tooLarge' });
  expect(load).not.toHaveBeenCalled();
});

it('bounds named-range matrices before ExcelJS workbook assignment', async () => {
  const files = await fixture(['<sheetData/>']);
  const book = new TextDecoder().decode(files['xl/workbook.xml']);
  files['xl/workbook.xml'] = new TextEncoder().encode(book.replace('</workbook>', '<definedNames><definedName name="Values">\'Sheet 1\'!$A$1:$C$2</definedName></definedNames></workbook>'));
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get');
  await expect(convertExcel(zipSync(files), 'xlsx', limits(4))).rejects.toMatchObject({ code: 'tooLarge' });
  expect(load).not.toHaveBeenCalled();
});

it.each([
  'Sheet1!$A$1:$C$2!$A$1',
  "'Sheet 1'!$A$1:$C$2!$A$1",
  "'A!B'!$A$1:$C$2!$A$1",
  "'A!B''s, 工作表'!$A$1:$C$2!$A$1",
  'Sheet1!$A$1:$C$2!$A$1:$A$1',
  'Sheet1&#33;$A$1:$C$2&#33;$A$1',
  'Sheet1!$A$1,Sheet1!$A$1:$C$2!$A$1',
])('rejects the complete ambiguous name reference %s before ExcelJS loading', async value => {
  // The largest possible expansion in these fixtures is only six cells.
  const bytes = await namedReference(value);
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get');
  await expect(convertExcel(bytes, 'xlsx', limits(4))).rejects.toMatchObject({ code: 'invalid' });
  expect(load).not.toHaveBeenCalled();
});

it('charges the complete rectangle after a quoted worksheet name containing an exclamation mark', async () => {
  const bytes = await namedReference("'A!B''s, 工作表'!$A$1:$C$2");
  const load = vi.spyOn(ExcelJS.Workbook.prototype, 'xlsx', 'get');
  await expect(convertExcel(bytes, 'xlsx', limits(4))).rejects.toMatchObject({ code: 'tooLarge' });
  expect(load).not.toHaveBeenCalled();
});

it('retains cached formulas using ordinary names, including quoted worksheet names', async () => {
  const workbook = new ExcelJS.Workbook(), sheet = workbook.addWorksheet("A,B's");
  sheet.getCell('A1').value = 42;
  sheet.getCell('B1').value = { formula: 'Values', result: 42 };
  workbook.definedNames.add("'A,B''s'!$A$1", 'Values');
  const bytes = new Uint8Array(await workbook.xlsx.writeBuffer());
  const preview = await convertExcel(bytes, 'xlsx', limits(3));
  expect(preview.sheets[0].celldata?.find(cell => cell.c === 1)?.v).toMatchObject({ v: 42, f: '=Values' });
});

it('retains quoted exclamation marks, commas and escaped apostrophes in worksheet names', async () => {
  const workbook = new ExcelJS.Workbook(), sheet = workbook.addWorksheet("A!B's, 工作表");
  sheet.getCell('A1').value = 42;
  sheet.getCell('B1').value = { formula: 'Values', result: 42 };
  workbook.definedNames.add("'A!B''s, 工作表'!$A$1", 'Values');
  const bytes = new Uint8Array(await workbook.xlsx.writeBuffer());
  const preview = await convertExcel(bytes, 'xlsx', limits(3));
  expect(preview.sheets[0].name).toBe("A!B's, 工作表");
  expect(preview.sheets[0].celldata?.find(cell => cell.c === 1)?.v).toMatchObject({ v: 42, f: '=Values' });
});

it('keeps ordinary merges and cached values at the exact aggregate budget', async () => {
  const xml = '<sheetData><row r="1"><c r="A1"><v>42</v></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:B2"/></mergeCells>';
  const bytes = zipSync(await fixture([xml, xml]));
  const preview = await convertExcel(bytes, 'xlsx', limits(8));
  expect(preview.sheets).toHaveLength(2);
  for (const sheet of preview.sheets) {
    expect(sheet.config?.merge).toEqual({ '0_0': { r: 0, c: 0, rs: 2, cs: 2 } });
    expect(sheet.celldata?.find(cell => cell.r === 0 && cell.c === 0)?.v?.v).toBe(42);
    expect([sheet.row, sheet.column]).toEqual([2, 2]);
  }
});
