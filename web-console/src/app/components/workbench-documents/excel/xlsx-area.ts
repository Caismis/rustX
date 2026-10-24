/** Bound worksheet allocations before ExcelJS builds rows, columns or merged cells. */
import { SaxesParser } from 'saxes'
import { ExcelPreviewError } from './error.ts'
import { XLSX_ZIP_LIMITS } from './xlsx-zip.ts'

const invalid = (): never => { throw new ExcelPreviewError('invalid') }
const tooLarge = (): never => { throw new ExcelPreviewError('tooLarge') }
const integer = (value: string | undefined, maximum: number): number => {
  if (!value || !/^[1-9][0-9]*$/.test(value)) return invalid()
  const result = Number(value)
  if (!Number.isSafeInteger(result) || result > maximum) return invalid()
  return result
}
const coordinate = (value: string): [number, number] => {
  const match = /^\$?([A-Z]{1,3})\$?([1-9][0-9]*)$/.exec(value)
  if (!match) return invalid()
  const row = integer(match[2], 1048576)
  const column = [...match[1]].reduce((value, letter) => value * 26 + letter.charCodeAt(0) - 64, 0)
  if (column > 16384) return invalid()
  return [row, column]
}

/** ExcelJS assigns worksheets[id] before its public worksheets getter clones
 * that sparse array. Bound identifiers independently of cell rectangles. */
export function admitWorkbookSheets(source: string, maxCells: number): void {
  if (!Number.isSafeInteger(maxCells) || maxCells < 1) tooLarge()
  const maximum = Math.min(maxCells, XLSX_ZIP_LIMITS.entries), ids = new Set<number>()
  let depth = 0
  const parser = new SaxesParser({ xmlns: false })
  parser.on('doctype', invalid)
  parser.on('error', invalid)
  parser.on('opentag', tag => {
    if (++depth > 64) invalid()
    // Match ExcelJS's unprefixed sheetId interpretation. Canonical decimal
    // values also prevent aliases such as 1, 01 and parseInt('1garbage').
    if (tag.name !== 'sheet') return
    const id = integer(tag.attributes.sheetId, Number.MAX_SAFE_INTEGER)
    if (id > maximum || ids.size >= maximum) tooLarge()
    if (ids.has(id)) invalid()
    ids.add(id)
  })
  parser.on('closetag', () => { depth-- })
  parser.write(source).close()
}

/** Return the actual rectangular extent; dimensions and declared counts are not authority. */
export function worksheetArea(source: string, maxCells: number): number {
  if (!Number.isSafeInteger(maxCells) || maxCells < 1) return tooLarge()
  let rows = 1, columns = 1, depth = 0
  const records = new Map<string, number>()
  const include = (row: number, column: number) => {
    rows = Math.max(rows, row)
    columns = Math.max(columns, column)
    if (rows * columns > maxCells) tooLarge()
  }
  const address = (value: string) => {
    include(...coordinate(value))
  }
  const range = (value: string | undefined) => {
    if (!value) return invalid()
    const endpoints = value.split(':')
    if (endpoints.length > 2) return invalid()
    endpoints.forEach(address)
  }
  // ExcelJS uses unprefixed element/attribute names; keep the same interpretation.
  const parser = new SaxesParser({ xmlns: false })
  parser.on('doctype', invalid)
  parser.on('error', invalid)
  parser.on('opentag', tag => {
    if (++depth > 64) invalid()
    switch (tag.name) {
      case 'row': include(integer(tag.attributes.r, 1048576), 1); break
      case 'c': {
        if (!tag.attributes.r) return invalid()
        address(tag.attributes.r)
        break
      }
      case 'col': {
        const first = integer(tag.attributes.min, 16384), last = integer(tag.attributes.max, 16384)
        if (first > last) invalid()
        include(1, last)
        break
      }
      case 'mergeCell': range(tag.attributes.ref); break
      // ExcelJS also expands validation ranges while loading worksheet XML.
      case 'dataValidation': {
        const ranges = tag.attributes.sqref
        if (!ranges?.trim()) return invalid()
        for (const match of ranges.matchAll(/\S+/g)) range(match[0])
        break
      }
      default: return
    }
    // Repeated coordinates must not create arbitrarily many intermediate records.
    const count = (records.get(tag.name) ?? 0) + 1
    if (count > maxCells) tooLarge()
    records.set(tag.name, count)
  })
  parser.on('closetag', () => { depth-- })
  parser.write(source).close()
  return rows * columns
}

/** Workbook names build separate cell matrices in ExcelJS, even without sheet cells. */
export function definedNamesArea(source: string, maxCells: number): number {
  let total = 0, text: string | undefined, depth = 0, records = 0
  const charge = (value: string) => {
    // Consume at most one complete sheet prefix, as ExcelJS does. Exclamation
    // marks inside quoted names belong to that prefix; none may remain in the
    // reference. Never validate only the tail after an extra separator.
    const parsed = /^(?:(?:'(?:[^']|'')*'|[^'^ !]+)!)?([^!]*)$/.exec(value)
    if (!parsed || parsed[0] !== value) return invalid()
    const ref = parsed[1]
    // This is the admission predicate used by ExcelJS DefinedNames.model.
    if (!/[$](\w+)[$](\d+)(:[$](\w+)[$](\d+))?/.test(ref)) return
    const ends = ref.split(':')
    if (ends.length > 2) return invalid()
    const [r1, c1] = coordinate(ends[0]), [r2, c2] = coordinate(ends[1] ?? ends[0])
    total += (Math.abs(r2 - r1) + 1) * (Math.abs(c2 - c1) + 1)
    if (total > maxCells) tooLarge()
  }
  const parser = new SaxesParser({ xmlns: false })
  parser.on('doctype', invalid)
  parser.on('error', invalid)
  parser.on('opentag', tag => {
    if (++depth > 64 || text !== undefined) return invalid()
    if (tag.name === 'definedName' && ++records > maxCells) tooLarge()
    if (tag.name === 'definedName' && !['_xlnm.Print_Area', '_xlnm.Print_Titles'].includes(tag.attributes.name)) text = ''
  })
  parser.on('text', value => { if (text !== undefined) text += value })
  parser.on('cdata', value => { if (text !== undefined) text += value })
  parser.on('closetag', tag => {
    depth--
    if (tag.name !== 'definedName' || text === undefined) return
    // Commas inside quoted worksheet names are not range separators.
    let quoted = false, start = 0
    for (let i = 0; i <= text.length; i++) {
      if (text[i] === "'") quoted = !quoted
      if (i === text.length || (text[i] === ',' && !quoted)) {
        charge(text.slice(start, i)); start = i + 1
      }
    }
    if (quoted) invalid()
    text = undefined
  })
  parser.write(source).close()
  return total
}
