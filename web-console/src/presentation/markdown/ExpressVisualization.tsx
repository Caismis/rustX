/**
 * Standalone renderer for express visualization nodes: a semantic themed
 * {@link DataTable} plus dependency-free SVG line/bar charts. Rows arrive as
 * plain reactive props; every figure recomputes its geometry from them.
 * Missing values stay missing (gaps in lines, absent bars, `—` cells);
 * negative and zero values keep their sign against a real zero baseline.
 */
import { useId, useMemo } from 'react';
import type { ReactNode } from 'react';
import clsx from 'clsx';
import { useTranslation } from '../../locale/react';
import { chartNumberFormatter, niceTicks, paddedDomain, r2, toNumber } from './express-chart-geometry';
import { ExpressPlot } from './ExpressPlot';
import plotCss from './ExpressPlot.module.css';
import css from './ExpressVisualization.module.css';

type ChartKind = 'LineChart' | 'BarChart' | 'AreaChart';

export interface ExpressVisualizationProps {
  kind: 'Table' | ChartKind;
  title?: string;
  rows: readonly Record<string, unknown>[];
  x?: string;
  y?: string | string[];
  columns?: string[];
  seriesLabels?: ReadonlyMap<string, string>;
  subtitle?: string;
  height?: number;
  format?: string;
  unit?: string;
}

const SERIES_COLORS = [
  'var(--a2-chart-1)',
  'var(--a2-chart-2)',
  'var(--a2-chart-3)',
  'var(--a2-chart-4)',
  'var(--a2-chart-5)',
  'var(--a2-chart-6)',
] as const;

const CHART_W = 640;
const CHART_H = 320;
const MARGIN = { top: 14, right: 14, bottom: 30, left: 52 } as const;
const PLOT_W = CHART_W - MARGIN.left - MARGIN.right;
const MAX_X_LABELS = 12;
/** Bars thinner than this still paint a visible sliver at the zero line. */
const MIN_BAR_H = 1.5;

/** Compact number for cells, ticks, and point titles. */
function formatNumber(value: number): string {
  if (!Number.isFinite(value)) return String(value);
  const abs = Math.abs(value);
  if (abs !== 0 && (abs >= 1e15 || abs < 1e-6)) return value.toExponential(2).replace(/\.?0+e/, 'e');
  return String(Number(value.toPrecision(12)));
}

/** Display string for a cell value; null marks a missing value. */
function formatScalar(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value === 'string') return value;
  if (typeof value === 'number') return formatNumber(value);
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  if (typeof value === 'bigint') return value.toString();
  try {
    const json = JSON.stringify(value);
    return json === undefined ? String(value) : json;
  } catch {
    return String(value);
  }
}

/** Column list: the explicit `columns` prop wins, else first-seen row keys. */
function rowColumns(rows: readonly Record<string, unknown>[], preferred?: readonly string[]): string[] {
  if (preferred !== undefined && preferred.length > 0) return [...new Set(preferred)];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const row of rows) {
    for (const key of Object.keys(row)) {
      if (!seen.has(key)) {
        seen.add(key);
        out.push(key);
      }
    }
  }
  return out;
}

/** True when every present value in the column is a finite number. */
function isNumericColumn(rows: readonly Record<string, unknown>[], column: string): boolean {
  let saw = false;
  for (const row of rows) {
    const value = row[column];
    if (value === null || value === undefined || value === '') continue;
    if (typeof value !== 'number' || !Number.isFinite(value)) return false;
    saw = true;
  }
  return saw;
}

function DataTable({ title, columns, rows }: { title?: string; columns: string[]; rows: readonly Record<string, unknown>[] }) {
  const tx = useTranslation();
  const numeric = useMemo(() => new Set(columns.filter(column => isNumericColumn(rows, column))), [columns, rows]);
  const scrollable = columns.length >= 4;
  return (
    <div className={css.tableScroll} tabIndex={scrollable ? 0 : undefined}
      role={scrollable ? 'region' : undefined}
      aria-label={scrollable ? (title || tx('interactions:express.table')) : undefined}>
      <table data-slot="table" aria-label={title ? undefined : tx('interactions:express.table')}>
        {title !== undefined && title !== '' && <caption className={css.caption}>{title}</caption>}
        {columns.length > 0 && (
          <thead>
            <tr>{columns.map(column => <th key={column} scope="col">{column}</th>)}</tr>
          </thead>
        )}
        <tbody>
          {rows.length === 0 || columns.length === 0
            ? <tr><td colSpan={Math.max(1, columns.length)} className={css.emptyTable}>{tx('interactions:express.no-data')}</td></tr>
            : rows.map((row, rowIndex) => (
              <tr key={rowIndex}>
                {columns.map(column => {
                  const text = formatScalar(row[column]);
                  return (
                    <td
                      key={column}
                      className={clsx(numeric.has(column) && css.numeric, text === null && css.missing)}
                      data-missing={text === null ? '' : undefined}
                    >
                      {text ?? '—'}
                    </td>
                  );
                })}
              </tr>
            ))}
        </tbody>
      </table>
    </div>
  );
}

function ExpressTable({ title, rows, columns }: { title?: string; rows: readonly Record<string, unknown>[]; columns?: string[] }) {
  const cols = useMemo(() => rowColumns(rows, columns), [rows, columns]);
  return <DataTable title={title} columns={cols} rows={rows} />;
}

interface PlotEntry {
  /** Category label (formatted x value, or 1-based position without `x`). */
  xText: string;
  /** Numeric x when the axis is numeric; otherwise null. */
  xNum: number | null;
  row: Record<string, unknown>;
}

interface ChartSeries {
  name: string;
  color: string;
  /** One slot per plot entry, in entry order; null marks a missing value. */
  values: (number | null)[];
}

interface ChartModel {
  entries: PlotEntry[];
  series: ChartSeries[];
  xNumeric: boolean;
  xDomain: [number, number] | null;
  yDomain: [number, number];
}

/**
 * Reduce rows + field names to plottable entries and series. Returns null
 * when nothing can be drawn (no rows, no usable x, or no numeric y data).
 * Entries sort by numeric x before series values align to them.
 */
function buildChartModel(kind: ChartKind, rows: readonly Record<string, unknown>[], x: string | undefined, y: string | string[] | undefined): ChartModel | null {
  const entries: PlotEntry[] = [];
  for (const [index, row] of rows.entries()) {
    if (x === undefined) {
      entries.push({ xText: String(index + 1), xNum: null, row });
      continue;
    }
    const value = row[x];
    if (value === null || value === undefined) continue;
    const text = formatScalar(value);
    if (text === null) continue;
    entries.push({ xText: text, xNum: toNumber(value), row });
  }
  if (entries.length === 0) return null;

  // A line chart gets a linear x scale only when every plottable x is numeric;
  // bars and mixed/categorical data always use evenly spaced slots.
  const xNumeric = kind !== 'BarChart' && x !== undefined && entries.every(entry => entry.xNum !== null);
  if (xNumeric) entries.sort((a, b) => (a.xNum ?? 0) - (b.xNum ?? 0));

  const explicit = typeof y === 'string' ? [y] : Array.isArray(y) ? y.filter((name): name is string => typeof name === 'string') : [];
  const names = explicit.length > 0 ? [...new Set(explicit)] : rowColumns(rows).filter(column => column !== x);
  const series = names
    .map((name, index) => ({
      name,
      color: SERIES_COLORS[index % SERIES_COLORS.length],
      values: entries.map(entry => toNumber(entry.row[name])),
    }))
    .filter(item => item.values.some(value => value !== null));
  if (series.length === 0) return null;

  const flat = series.flatMap(item => item.values.filter((value): value is number => value !== null));
  const yLo = Math.min(...flat);
  const yHi = Math.max(...flat);
  let yDomain: [number, number];
  let xDomain: [number, number] | null = null;
  if (kind === 'BarChart') {
    // Bars need a real zero baseline even when every value shares one sign.
    yDomain = paddedDomain(Math.min(0, yLo), Math.max(0, yHi), 0.05);
  } else {
    yDomain = paddedDomain(yLo, yHi, 0.1);
    if (xNumeric) {
      const xs = entries.map(entry => entry.xNum ?? 0);
      xDomain = paddedDomain(Math.min(...xs), Math.max(...xs), 0.05);
    }
  }
  return { entries, series, xNumeric, xDomain, yDomain };
}

interface Point {
  x: number;
  y: number;
  value: number;
  xText: string;
}

/** Contiguous runs of non-null values; each run becomes one path plus dots. */
function lineSegments(model: ChartModel, yScale: (v: number) => number, xAt: (index: number, entry: PlotEntry) => number): Point[][][] {
  return model.series.map(item => {
    const segments: Point[][] = [];
    let current: Point[] = [];
    model.entries.forEach((entry, index) => {
      const value = item.values[index];
      if (value === null) {
        if (current.length > 0) segments.push(current);
        current = [];
        return;
      }
      current.push({ x: xAt(index, entry), y: yScale(value), value, xText: entry.xText });
    });
    if (current.length > 0) segments.push(current);
    return segments;
  });
}

function ExpressChart({ kind, title, rows, x, y, seriesLabels, subtitle, height, format, unit }: Omit<ExpressVisualizationProps, 'kind'> & { kind: ChartKind }) {
  const id = useId();
  const tx = useTranslation();
  const model = useMemo(() => buildChartModel(kind, rows, x, y), [kind, rows, x, y]);
  const kindLabel = tx(kind === 'BarChart' ? 'interactions:express.bar-chart'
    : kind === 'AreaChart' ? 'interactions:express.area-chart' : 'interactions:express.line-chart');
  const chartHeight = height === undefined ? CHART_H : Math.max(150, Math.min(440, height));
  const plotH = chartHeight - MARGIN.top - MARGIN.bottom;
  const displayNumber = format || unit ? chartNumberFormatter(format, unit) : formatNumber;
  const displayTick = format || unit ? chartNumberFormatter(format, unit, true) : formatNumber;
  const label = (key: string) => seriesLabels?.get(key) ?? key;

  let body: ReactNode = <p className={css.empty}>{tx('interactions:express.no-data')}</p>;
  if (model !== null) {
    const count = model.entries.length;
    const [yLo, yHi] = model.yDomain;
    const ySpan = yHi - yLo;
    const yScale = (value: number) => MARGIN.top + (1 - (value - yLo) / ySpan) * plotH;
    const numeric = model.xNumeric && model.xDomain !== null;
    const [xLo, xHi] = model.xDomain ?? [0, 1];
    const xSpan = xHi - xLo;
    const xAt = (index: number, entry: PlotEntry) => numeric
      ? MARGIN.left + ((entry.xNum ?? 0) - xLo) / xSpan * PLOT_W
      : MARGIN.left + ((index + 0.5) / count) * PLOT_W;
    const yTicks = niceTicks(yLo, yHi);
    const zeroY = yScale(0);
    const showZero = yLo < 0 && yHi > 0;
    const labelStep = Math.max(1, Math.ceil(count / MAX_X_LABELS));
    const segments = kind === 'BarChart' ? null : lineSegments(model, yScale, xAt);

    // Grouped bars: one band per entry, series side by side inside it.
    const slotW = PLOT_W / count;
    const groupW = slotW * 0.72;
    const barGap = model.series.length > 1 ? Math.min(3, groupW * 0.08) : 0;
    const barW = Math.max(0.5, (groupW - barGap * (model.series.length - 1)) / model.series.length);

    const seriesNames = model.series.map(item => label(item.name)).join(', ');
    const description = tx('interactions:express.chart-summary', { series: seriesNames, count });

    body = (
      <ExpressPlot label={title || kindLabel} positions={model.entries.map((entry, index) => xAt(index, entry) / CHART_W)}
        details={index => ({
          title: model.entries[index].xText,
          rows: model.series.flatMap(item => {
            const value = item.values[index];
            return value === null ? [] : [{ label: label(item.name), value: displayNumber(value), color: item.color }];
          }),
        })}>
      {active => <svg
        className={css.chart}
        viewBox={`0 0 ${CHART_W} ${chartHeight}`}
        role="img"
        aria-labelledby={`${id}-title`}
        aria-describedby={`${id}-desc`}
      >
        <title id={`${id}-title`}>{title || kindLabel}</title>
        <desc id={`${id}-desc`}>{description}</desc>
        {active !== undefined && <line className={plotCss.guide} x1={xAt(active, model.entries[active])}
          x2={xAt(active, model.entries[active])} y1={MARGIN.top} y2={MARGIN.top + plotH}/>}
        {yTicks.map(tick => (
          <g key={tick}>
            <line className={css.grid} x1={MARGIN.left} x2={MARGIN.left + PLOT_W} y1={r2(yScale(tick))} y2={r2(yScale(tick))} />
            <text className={css.tick} x={MARGIN.left - 8} y={r2(yScale(tick))} textAnchor="end" dy="0.35em">{displayTick(tick)}</text>
          </g>
        ))}
        {showZero && <line className={css.zero} data-part="zero" x1={MARGIN.left} x2={MARGIN.left + PLOT_W} y1={r2(zeroY)} y2={r2(zeroY)} />}
        <line className={css.axis} data-part="axis" x1={MARGIN.left} x2={MARGIN.left + PLOT_W} y1={MARGIN.top + plotH} y2={MARGIN.top + plotH} />
        {kind !== 'BarChart' && model.series.map((item, seriesIndex) => {
          const runs = segments?.[seriesIndex] ?? [];
          return (
            <g key={`${item.name}-${seriesIndex}`}>
              {runs.filter(run => run.length > 1).map((run, runIndex) => {
                const path = run.map((point, pointIndex) => `${pointIndex === 0 ? 'M' : 'L'}${r2(point.x)} ${r2(point.y)}`).join(' ');
                const bottom = r2(yScale(Math.max(yLo, Math.min(0, yHi))));
                return <g key={runIndex}>
                  {kind === 'AreaChart' && <path data-part="area" fill={item.color} fillOpacity={0.22}
                    d={`${path} L${r2(run.at(-1)!.x)} ${bottom} L${r2(run[0].x)} ${bottom} Z`}/>}
                  <path
                  className={css.line}
                  data-part="line"
                  style={{ stroke: item.color }}
                  d={path}
                />
                </g>;
              })}
              {runs.flatMap((run, runIndex) => run.map((point, pointIndex) => (
                <circle
                  key={`${runIndex}-${pointIndex}`}
                  className={css.point}
                  data-part="point"
                  style={{ fill: item.color }}
                  cx={r2(point.x)}
                  cy={r2(point.y)}
                  r={3}
                >
                  <title>{`${point.xText} · ${label(item.name)}: ${displayNumber(point.value)}`}</title>
                </circle>
              )))}
            </g>
          );
        })}
        {kind === 'BarChart' && model.series.map((item, seriesIndex) => (
          <g key={`${item.name}-${seriesIndex}`}>
            {model.entries.map((entry, entryIndex) => {
              const value = item.values[entryIndex];
              if (value === null) return null;
              const valueY = yScale(value);
              let top = Math.min(valueY, zeroY);
              let height = Math.abs(valueY - zeroY);
              if (height < MIN_BAR_H) {
                if (value > 0) top = zeroY - MIN_BAR_H;
                else if (value < 0) top = zeroY;
                else top = zeroY - MIN_BAR_H / 2;
                height = MIN_BAR_H;
              }
              const slotX = MARGIN.left + entryIndex * slotW + (slotW - groupW) / 2;
              return (
                <rect
                  key={entryIndex}
                  className={css.bar}
                  data-part="bar"
                  style={{ fill: item.color }}
                  x={r2(slotX + seriesIndex * (barW + barGap))}
                  y={r2(top)}
                  width={r2(barW)}
                  height={r2(height)}
                >
                  <title>{`${entry.xText} · ${label(item.name)}: ${displayNumber(value)}`}</title>
                </rect>
              );
            })}
          </g>
        ))}
        {numeric
          ? niceTicks(xLo, xHi).map(tick => (
            <text
              key={tick}
              className={css.tick}
              x={r2(MARGIN.left + (tick - xLo) / xSpan * PLOT_W)}
              y={chartHeight - MARGIN.bottom + 15}
              textAnchor="middle"
            >
              {formatNumber(tick)}
            </text>
          ))
          : model.entries.map((entry, index) => (index % labelStep === 0 || index === count - 1) && (
            <text
              key={index}
              className={css.tick}
              x={r2(xAt(index, entry))}
              y={chartHeight - MARGIN.bottom + 15}
              textAnchor="middle"
            >
              {entry.xText}
            </text>
          ))}
      </svg>}
      </ExpressPlot>
    );
  }

  return (
    <figure className={css.figure}>
      {(title || subtitle) && <figcaption className={css.title}>
        {title}{subtitle && <span className={css.subtitle}>{subtitle}</span>}
      </figcaption>}
      {body}
      {model !== null && (
        <ul className={css.legend}>
          {model.series.map((item, index) => (
            <li key={`${item.name}-${index}`}>
              <span className={css.swatch} style={{ background: item.color }} />
              {label(item.name)}
            </li>
          ))}
        </ul>
      )}
    </figure>
  );
}

export function ExpressVisualization(props: ExpressVisualizationProps) {
  if (props.kind === 'Table') return <ExpressTable title={props.title} rows={props.rows} columns={props.columns} />;
  return <ExpressChart {...props} kind={props.kind}/>;
}

export function PlaygroundChart({ kind, props }: { kind: ChartKind; props: Record<string, unknown> }) {
  const tx = useTranslation();
  const valueLabel = tx('interactions:express.value');
  const pivoted = useMemo(() => {
    const rows = new Map<string | number, Record<string, unknown>>();
    const labels = new Map<string, string>();
    if (Array.isArray(props.data)) {
      for (const item of props.data) {
        if (!item || typeof item !== 'object' || Array.isArray(item)) continue;
        const { x, y, series } = item;
        if ((typeof x !== 'string' && toNumber(x) === null) || toNumber(y) === null) continue;
        const name = typeof series === 'string' ? series : undefined;
        const key = name === undefined ? 'default' : `series:${name}`;
        labels.set(key, name ?? valueLabel);
        const row = rows.get(x) ?? Object.assign(Object.create(null), { x });
        row[key] = y;
        rows.set(x, row);
      }
    }
    return { rows: [...rows.values()], y: [...labels.keys()], labels };
  }, [props.data, valueLabel]);
  return <ExpressChart kind={kind} rows={pivoted.rows} x="x" y={pivoted.y} seriesLabels={pivoted.labels}
    title={typeof props.title === 'string' ? props.title : undefined}
    subtitle={typeof props.subtitle === 'string' ? props.subtitle : undefined}
    height={toNumber(props.height) ?? undefined} format={typeof props.format === 'string' ? props.format : undefined}
    unit={typeof props.unit === 'string' ? props.unit : undefined}/>;
}
