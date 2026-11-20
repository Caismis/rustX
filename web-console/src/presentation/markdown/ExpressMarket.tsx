import { useId } from 'react';
import { useTranslation } from '../../locale/react';
import { BundledIcon } from '../primitives/BundledIcon';
import { scalarText } from './express-values';
import { chartNumberFormatter, niceTicks, paddedDomain, r2, toNumber } from './express-chart-geometry';
import { ExpressPlot } from './ExpressPlot';
import plotCss from './ExpressPlot.module.css';
import chartCss from './ExpressVisualization.module.css';
import css from './ExpressMarket.module.css';

type Props = { props: Record<string, unknown> };
type Stat = { label: string; value: string; hint: string };

function rows(value: unknown): Record<string, unknown>[] {
  return Array.isArray(value) ? value.filter(row => row !== null && typeof row === 'object' && !Array.isArray(row)) : [];
}

function stats(value: unknown): Stat[] {
  return rows(value).flatMap(item => typeof item.label === 'string'
    ? [{ label: item.label, value: scalarText(item.value), hint: scalarText(item.hint) }] : []);
}

function currencyFormatter(currency: unknown, digits: number) {
  const options = { minimumFractionDigits: digits, maximumFractionDigits: 2 };
  const code = typeof currency === 'string' ? currency : 'USD';
  return new Intl.NumberFormat(undefined, /^[a-z]{3}$/i.test(code) ? { ...options, style: 'currency', currency: code } : options);
}

const signedNumber = new Intl.NumberFormat(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2, signDisplay: 'exceptZero' });
const signedPercent = new Intl.NumberFormat(undefined, { style: 'percent', minimumFractionDigits: 2, maximumFractionDigits: 2, signDisplay: 'exceptZero' });

function StatList({ entries, columns, ticker = false }: { entries: Stat[]; columns?: number; ticker?: boolean }) {
  return <dl className={ticker ? css.quoteStats : css.keyValues} data-columns={columns}>
    {entries.map(({ label, value, hint }, index) => <div key={index}>
      <dt>{label}</dt>
      <dd>{value}{hint && <small>{hint}</small>}</dd>
    </div>)}
  </dl>;
}

function Sparkline({ values, format }: { values: number[]; format: (value: number) => string }) {
  const id = useId();
  const tx = useTranslation();
  if (values.length < 2) return null;
  const min = Math.min(...values);
  const max = Math.max(...values);
  const range = max - min;
  if (!Number.isFinite(range)) return null;
  const width = 640;
  const height = 72;
  const yAt = (value: number) => range ? r2(5 + (1 - (value - min) / range) * 60) : height / 2;
  const line = values.map((value, index) => `${index === 0 ? 'M' : 'L'}${r2(index * width / (values.length - 1))} ${yAt(value)}`).join(' ');
  const xAt = (index: number) => index * width / (values.length - 1);
  return <ExpressPlot label={tx('interactions:express.price-trend')} positions={values.map((_, index) => index / (values.length - 1))}
    details={index => ({ title: format(values[index]), rows: [] })}>
    {active => <svg className={css.sparkline} viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" role="img" aria-label={tx('interactions:express.price-trend')}>
    <defs>
      <linearGradient id={id} x2="0" y2="1">
        <stop offset="0%" stopColor="currentColor" stopOpacity=".24"/>
        <stop offset="100%" stopColor="currentColor" stopOpacity="0"/>
      </linearGradient>
    </defs>
    <line className={chartCss.grid} x1={0} x2={width} y1={yAt(values[0])} y2={yAt(values[0])}/>
    <path d={`${line} L${width} ${height} H0 Z`} fill={`url(#${id})`}/>
    <path className={chartCss.line} d={line} data-part="sparkline" stroke="currentColor"/>
    {active !== undefined && <>
      <line className={plotCss.guide} x1={xAt(active)} x2={xAt(active)} y1={0} y2={height}/>
      <circle cx={xAt(active)} cy={yAt(values[active])} r={3} fill="currentColor"/>
    </>}
    </svg>}
  </ExpressPlot>;
}

function numbers(value: unknown): number[] {
  return Array.isArray(value) ? value.filter((item): item is number => toNumber(item) !== null) : [];
}

export function ExpressStockTicker({ props }: Props) {
  const tx = useTranslation();
  const symbol = scalarText(props.symbol);
  const name = scalarText(props.name);
  const price = toNumber(props.price);
  const change = toNumber(props.change);
  const percent = toNumber(props.changePct);
  const values = numbers(props.series);
  const delta = change ?? percent ?? (values.length > 1 ? values.at(-1)! - values[0] : 0);
  const entries = stats(props.stats);
  const formatter = currencyFormatter(props.currency, 2);
  const formattedPrice = price === null ? '—' : formatter.format(price);
  const formattedChange = change === null ? '' : signedNumber.format(change);
  const formattedPercent = percent === null ? '' : signedPercent.format(percent / 100);
  return <figure className={`${chartCss.figure} ${css.ticker}`} data-trend={delta < 0 ? 'down' : delta > 0 ? 'up' : 'flat'}>
    <figcaption>
      <div className={css.quoteHeader}>
        <div className={css.identity}>
          <span className={css.tickerMark} aria-hidden="true">{symbol.slice(0, 4)}</span>
          <div>
            <strong className={css.symbol}>{symbol}</strong>
            {name && <div className={css.muted}>{name}</div>}
          </div>
        </div>
        {props.sample === true && <span className={css.sample}>{tx('interactions:express.sample-data')}</span>}
      </div>
      <div className={css.quote}>
        <strong>{formattedPrice}</strong>
        {(formattedChange || formattedPercent) && <span className={css.change}>
          <span aria-hidden="true">{delta < 0 ? '↘' : delta > 0 ? '↗' : '–'}</span>
          {formattedChange && formattedPercent ? `${formattedChange} (${formattedPercent})` : formattedChange || formattedPercent}
        </span>}
      </div>
    </figcaption>
    <Sparkline values={values} format={formatter.format}/>
    {entries.length > 0 && <StatList entries={entries} ticker/>}
  </figure>;
}

function ohlc(value: unknown) {
  return rows(value).flatMap(row => {
    const { date } = row;
    const open = toNumber(row.open), high = toNumber(row.high), low = toNumber(row.low), close = toNumber(row.close);
    if (typeof date !== 'string' || open === null || high === null || low === null || close === null) return [];
    return [{ date, open, high: Math.max(high, open, close), low: Math.min(low, open, close), close }];
  });
}

export function ExpressCandlestick({ props }: Props) {
  const tx = useTranslation();
  const data = ohlc(props.data);
  const title = scalarText(props.title);
  const subtitle = scalarText(props.subtitle);
  const [lo, hi] = data.length
    ? paddedDomain(Math.min(...data.map(candle => candle.low)), Math.max(...data.map(candle => candle.high)), .07)
    : [0, 0];
  const span = hi - lo;
  const last = data.at(-1);
  const firstOpen = data[0]?.open;
  const change = last && firstOpen ? (last.close - firstOpen) / firstOpen : undefined;
  const percent = change !== undefined && Number.isFinite(change) ? signedPercent.format(change) : undefined;
  const value = chartNumberFormatter(props.format, props.unit);
  const tickValue = chartNumberFormatter(props.format, props.unit, true);
  const width = 680;
  const height = Math.max(150, Math.min(440, toNumber(props.height) ?? 240));
  const left = 57, right = width - 20, top = 14, bottom = height - 26;
  const step = data.length ? (right - left) / data.length : 0;
  const candleWidth = Math.min(26, step * .62);
  const xAt = (index: number) => r2(left + (index + .5) * step);
  const yAt = (price: number) => r2(top + (1 - (price - lo) / span) * (bottom - top));
  const labelStep = Math.max(1, Math.ceil((data.length - 1) / 10));
  const labels = data.map((candle, index) => ({ ...candle, index })).filter(({ index }) => (
    index === data.length - 1 || index % labelStep === 0 && index + labelStep * .65 < data.length - 1
  ));

  return <figure className={chartCss.figure}>
    <figcaption className={css.candleCaption}>
      <div>
        <strong>{title || tx('interactions:express.candlestick-chart')}</strong>
        {subtitle && <div className={css.muted}>{subtitle}</div>}
      </div>
      {last && <div className={css.candleSummary}>
        <strong>{value(last.close)}</strong>
        {change !== undefined && percent && <div className={css.change} data-trend={change < 0 ? 'down' : change > 0 ? 'up' : 'flat'}>
          {tx('interactions:express.period-change', { change: percent })}
        </div>}
      </div>}
    </figcaption>
    {last && Number.isFinite(span) && span > 0 ? <ExpressPlot
      label={title || tx('interactions:express.candlestick-chart')}
      positions={data.map((_, index) => xAt(index) / width)}
      details={index => {
        const candle = data[index];
        const trend = candle.close >= candle.open ? 'var(--a2-positive)' : 'var(--a2-negative)';
        const percent = candle.open === 0 ? undefined : (candle.close - candle.open) / candle.open;
        return {
          title: candle.date,
          rows: [
            { label: tx('interactions:express.open'), value: value(candle.open) },
            { label: tx('interactions:express.high'), value: value(candle.high) },
            { label: tx('interactions:express.low'), value: value(candle.low) },
            { label: tx('interactions:express.close'), value: value(candle.close), color: trend },
            ...(percent !== undefined && Number.isFinite(percent) ? [{
              label: tx('interactions:express.change'),
              value: signedPercent.format(percent), color: trend, emphasis: true,
            }] : []),
          ],
        };
      }}>
      {active => <svg className={`${chartCss.chart} ${css.candlePlot}`}
      viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none"
      role="img" aria-label={title || tx('interactions:express.candlestick-chart')}>
      <desc>{tx('interactions:express.candle-count', { count: data.length })}</desc>
      {niceTicks(lo, hi).map(tick => <g key={tick}>
        <line className={chartCss.grid} x1={left} x2={right} y1={yAt(tick)} y2={yAt(tick)}/>
        <text className={chartCss.tick} x={left - 8} y={yAt(tick)} textAnchor="end" dy=".35em">{tickValue(tick)}</text>
      </g>)}
      {active !== undefined && <line className={plotCss.guide} x1={xAt(active)} x2={xAt(active)} y1={top} y2={bottom}/>}
      {data.map((candle, index) => {
        const { date, open, high, low, close } = candle;
        const openY = yAt(open), closeY = yAt(close), x = xAt(index);
        const body = Math.max(Math.abs(closeY - openY), 1.4);
        return <g key={index} data-part="candle" data-active={index === active || undefined} className={css.candle} data-trend={close < open ? 'down' : 'up'}>
          <title>{[
            date,
            `${tx('interactions:express.open')} ${value(open)}`,
            `${tx('interactions:express.high')} ${value(high)}`,
            `${tx('interactions:express.low')} ${value(low)}`,
            `${tx('interactions:express.close')} ${value(close)}`,
          ].join(' · ')}</title>
          <line x1={x} x2={x} y1={yAt(high)} y2={yAt(low)} stroke="currentColor" vectorEffect="non-scaling-stroke"/>
          <rect x={r2(x - candleWidth / 2)} width={r2(candleWidth)} y={r2((openY + closeY - body) / 2)} height={r2(body)} rx={1} fill="currentColor"/>
        </g>;
      })}
      {labels.map(({ date, index }, position) => <text className={chartCss.tick} key={index}
        data-intermediate={position % 2 === 1 && position !== labels.length - 1 || undefined}
        x={xAt(index)} y={bottom + 16} textAnchor="middle">{date.length > 10 ? `${date.slice(0, 9)}…` : date}</text>)}
    </svg>}
    </ExpressPlot> : <p className={chartCss.empty}>{tx('interactions:express.no-data')}</p>}
  </figure>;
}

export function ExpressKeyValues({ props }: Props) {
  const title = scalarText(props.title);
  const columns = Math.max(1, Math.min(3, Math.round(toNumber(props.columns) ?? 1)));
  return <div className={chartCss.figure}>
    {title && <strong>{title}</strong>}
    <StatList entries={stats(props.items)} columns={columns}/>
  </div>;
}

const alertIcons: Readonly<Record<string, string>> = {
  info: 'info', success: 'circle-check', warning: 'triangle-alert', error: 'circle-x',
};

export function ExpressAlert({ props }: Props) {
  const key = scalarText(props.tone);
  const tone = Object.hasOwn(alertIcons, key) ? key : 'info';
  const message = scalarText(props.message);
  return <aside className={css.alert} data-tone={tone}>
    <BundledIcon name={`lucide:${alertIcons[tone]}`} size={16} fallback={null}/>
    <div>
      <strong>{scalarText(props.title)}</strong>
      {message && <p>{message}</p>}
    </div>
  </aside>;
}

export function UnsupportedExpressComponent({ name }: { name: string }) {
  const tx = useTranslation();
  return <div className={css.alert}>{tx('interactions:express.unsupported-component', { name })}</div>;
}

function StatValue({ props }: Props) {
  const delta = scalarText(props.delta);
  const caption = scalarText(props.caption);
  const trend = scalarText(props.trend);
  return <div className={css.stat}>
    <span className={css.muted}>{scalarText(props.label)}</span>
    <div className={css.statValue}>
      <strong>{scalarText(props.value)}</strong>
      {delta && <span className={css.change} data-trend={trend}>{delta}</span>}
    </div>
    {caption && <span className={css.muted}>{caption}</span>}
  </div>;
}

export function ExpressStat({ props }: Props) {
  return <div className={chartCss.figure}><StatValue props={props}/></div>;
}

export function ExpressStatGroup({ props }: Props) {
  return <div className={`${chartCss.figure} ${css.statGroup}`}>
    {rows(props.items).map((item, index) => <StatValue key={index} props={item}/>)}
  </div>;
}

export function ExpressSparkline({ props }: Props) {
  const values = numbers(props.values);
  const tone = props.tone === 'positive' ? 'up' : props.tone === 'negative' ? 'down' : 'flat';
  return <figure className={`${chartCss.figure} ${css.ticker}`} data-trend={tone}>
    <figcaption className={css.quoteHeader}><StatValue props={props}/></figcaption>
    <Sparkline values={values} format={new Intl.NumberFormat().format}/>
  </figure>;
}
