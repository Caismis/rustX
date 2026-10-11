/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
// Adapted from DeepSeek Harness ui-chat TurnUsagePanel and StatsPills
// (Detailed mode) and ui-conversation ContextMeter: the completed Turn's usage
// pill, and the session activity, token usage and context occupancy pills
// under the composer. Every figure is a native rustX reading; the browser
// tokenizes and times nothing. Harness's Compact mode, plugin dock slots and
// cache-write bucket have no rustX counterpart and are excluded.
import type { ContextOccupancy, ConversationStatistics, ModelUsage } from '../../../../protocol/app-server/v39';
import type { Translate } from '../../locale/translation';
import { useTranslation } from '../../locale/react';
import { IconDatabaseOutline16, IconGaugeOutline16 } from '../../presentation/primitives/icons';
import { StatDialog, StatHeading, statCss } from '../../presentation/primitives/StatDialog';
import { formatCacheHitPercent, formatDuration, formatExactTokens, formatTokens, formatTokensPerSecond } from './token-format';
import css from './UsageStats.module.css';

const count = (tx: Translate, value: number) => tx('agent:usage.count', { count: formatTokens(value, tx) });
const exact = (tx: Translate, value: number) => tx('agent:usage.count', { count: formatExactTokens(value, tx) });

/** Input buckets of one usage: uncached and cached when the provider split them. */
function inputRows(tx: Translate, usage: ModelUsage) {
  const cached = usage.details?.cached_input_tokens;
  if (cached == null || cached > usage.input_tokens) return <><dt>{tx('agent:usage.input-total')}</dt><dd>{exact(tx, usage.input_tokens)}</dd></>;
  return <>
    <dt>{tx('agent:usage.input')}</dt><dd>{exact(tx, usage.input_tokens - cached)}</dd>
    <dt>{tx('agent:usage.cache-read')}</dt><dd>{exact(tx, cached)}</dd>
  </>;
}
function cacheHit(usage: ModelUsage, decimalPlaces: 0 | 1) {
  const cached = usage.details?.cached_input_tokens;
  return cached == null || cached > usage.input_tokens ? null : formatCacheHitPercent(cached, usage.input_tokens, decimalPlaces);
}

/** The completed Turn's usage pill and its accounting dialog. */
export function TurnUsage({ usage, models }: { usage: ModelUsage; models: readonly string[] }) {
  const tx = useTranslation();
  const hit = cacheHit(usage, 1);
  const reasoning = usage.details?.reasoning_tokens;
  const consumed = tx('agent:usage.consumed', { total: count(tx, usage.total_tokens) });
  // The narrow layout hides the words, so the name never rides on them.
  return <StatDialog triggerClassName={css.trigger} label={consumed} title={tx('agent:usage.turn-title')}
    trigger={<><IconDatabaseOutline16/><span className={css.label}>{consumed}</span></>}>
    <StatHeading icon={<IconDatabaseOutline16/>} title={tx('agent:usage.turn-title')} value={exact(tx, usage.total_tokens)}/>
    <dl className={statCss.details} data-turn-usage-details="">
      {models.length > 0 && <><dt>{tx('agent:usage.model')}</dt><dd className={statCss.route}>{models.join(', ')}</dd></>}
      {hit !== null && <><dt>{tx('agent:usage.cache-hit')}</dt><dd>{`${hit}%`}</dd></>}
      {inputRows(tx, usage)}
      <dt>{tx('agent:usage.output')}</dt>
      <dd>{exact(tx, usage.output_tokens)}{reasoning != null && <span className={statCss.reasoning}>{tx('agent:usage.reasoning', { tokens: exact(tx, reasoning) })}</span>}</dd>
    </dl>
  </StatDialog>;
}

function Joined({ first, second }: { first: string; second: string | null }) {
  return second === null ? <>{first}</> : <>{first}<span className={css.sep} aria-hidden="true">·</span>{second}</>;
}

/** Turn and Step counts with whole-session speed, opening the time and speed dialog. */
function ActivityPill({ statistics }: { statistics: ConversationStatistics }) {
  const tx = useTranslation();
  const steps = Number(statistics.steps);
  if (!steps) return null;
  const timing = statistics.timing;
  const speed = timing?.output_tokens_per_second != null ? tx('agent:usage.tps', { tps: formatTokensPerSecond(timing.output_tokens_per_second) }) : null;
  const counts = tx('agent:usage.counts', { turns: statistics.turns, steps: statistics.steps });
  const model = timing?.model_ms ?? 0, tools = timing?.tool_ms ?? 0, ttft = timing?.mean_ttft_ms;
  const content = <><IconGaugeOutline16/><span className={css.label}><Joined first={counts} second={speed}/></span></>;
  // Without one timed figure the dialog would be empty, so the pill stays a plain reading.
  if (model <= 0 && tools <= 0 && ttft == null && speed === null) return <span className={css.pill} data-composer-stat="activity">{content}</span>;
  return <StatDialog triggerClassName={css.pill} label={speed === null ? counts : `${counts} · ${speed}`} title={tx('agent:usage.session-title')} trigger={content}>
    <StatHeading icon={<IconGaugeOutline16/>} title={tx('agent:usage.session-title')}/>
    <dl className={statCss.details} data-session-stats-details="">
      {model > 0 && <><dt>{tx('agent:usage.llm-time')}</dt><dd>{formatDuration(model, tx)}</dd></>}
      {tools > 0 && <><dt>{tx('agent:usage.tool-time')}</dt><dd>{formatDuration(tools, tx)}</dd></>}
      {ttft != null && <><dt>{tx('agent:usage.ttft')}</dt><dd>{formatDuration(ttft, tx)}</dd></>}
      {speed !== null && <><dt>{tx('agent:usage.speed')}</dt><dd>{speed}</dd></>}
    </dl>
  </StatDialog>;
}

/** Whole-conversation reported tokens and cache hit. */
function UsagePill({ statistics }: { statistics: ConversationStatistics }) {
  const tx = useTranslation();
  const usage = statistics.reported_usage;
  if (!usage || (usage.input_tokens === 0 && usage.output_tokens === 0)) return null;
  const hit = cacheHit(usage, 0);
  const hitText = hit === null ? null : tx('agent:usage.cache-hit-percent', { percent: hit });
  const total = count(tx, usage.total_tokens);
  return <StatDialog triggerClassName={css.pill} label={hitText === null ? total : `${total} · ${hitText}`} title={tx('agent:usage.usage-title')}
    trigger={<><IconDatabaseOutline16/><span className={css.label}><Joined first={total} second={hitText}/></span></>}>
    <StatHeading icon={<IconDatabaseOutline16/>} title={tx('agent:usage.usage-title')} value={exact(tx, usage.total_tokens)}/>
    <dl className={statCss.details} data-session-stats-usage="">
      {hit !== null && <><dt>{tx('agent:usage.cache-hit')}</dt><dd>{`${hit}%`}</dd></>}
      {inputRows(tx, usage)}
      <dt>{tx('agent:usage.output')}</dt><dd>{exact(tx, usage.output_tokens)}</dd>
      {/* Coverage is explicit: requests without a usage report are missing, not zero. */}
      {statistics.requests_with_usage !== statistics.model_requests && <><dt>{tx('agent:usage.reports')}</dt><dd>{`${statistics.requests_with_usage}/${statistics.model_requests}`}</dd></>}
    </dl>
  </StatDialog>;
}

/** Ring geometry: 14px viewBox, 2px stroke. */
const RADIUS = 5.5;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;
/** Splits the localized occupancy sentence so each locale keeps its word order. */
const READING_SLOT = '\u0000';
const ROWS = [
  { key: 'system_tokens', label: 'agent:usage.context-system', color: css.colorSystem },
  { key: 'tool_tokens', label: 'agent:usage.context-tools', color: css.colorTools },
  { key: 'message_tokens', label: 'agent:usage.context-messages', color: css.colorMessages },
] as const;

/** The last measured request's context occupancy, with its estimated composition. */
function ContextMeter({ occupancy }: { occupancy: ContextOccupancy }) {
  const tx = useTranslation();
  if (!(occupancy.context_window_tokens > 0)) return null;
  const percent = Math.min(100, Math.round(occupancy.input_tokens / occupancy.context_window_tokens * 100));
  const reading = `${percent}%`;
  const [before = '', after = ''] = tx('agent:usage.context-aria', { percent: READING_SLOT }).split(READING_SLOT).map(part => part.trim());
  const breakdown = occupancy.breakdown;
  const parts = breakdown.system_tokens + breakdown.tool_tokens + breakdown.message_tokens;
  // The bar's length stays the measured percent; the estimate only proportions its parts.
  const segments = (parts === 0 ? [{ key: 'total', color: undefined, width: percent }]
    : ROWS.map(row => ({ key: row.key, color: row.color, width: percent * breakdown[row.key] / parts }))).filter(part => part.width > 0);
  return <StatDialog triggerClassName={css.meter} label={tx('agent:usage.context-aria', { percent: reading })} title={tx('agent:usage.context-used')} panelClassName={css.contextPanel}
    trigger={<><svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true">
      <circle className={css.track} cx="7" cy="7" r={RADIUS}/>
      <circle className={css.fill} cx="7" cy="7" r={RADIUS} strokeDasharray={`${CIRCUMFERENCE * percent / 100} ${CIRCUMFERENCE}`} transform="rotate(-90 7 7)"/>
    </svg><span>{reading}</span></>}>
    <div className={css.header}>
      <span className={css.headline}>{before}</span>
      <span className={css.percent}>{reading}</span>
      <span className={css.headline}>{after}</span>
      {/* The provider measured the numerator exactly; only the parts below are estimates. */}
      <span className={css.figures}>{`${formatTokens(occupancy.input_tokens, tx)} / ${formatTokens(occupancy.context_window_tokens, tx)}`}</span>
    </div>
    <div className={css.bar}>{segments.map(segment => <div key={segment.key} className={segment.color ? `${css.segment} ${segment.color}` : css.segment} style={{ width: `${segment.width}%` }}/>)}</div>
    <dl className={css.rows}>
      {ROWS.map(row => <div key={row.key} className={css.row}>
        <dt><span className={`${css.swatch} ${row.color}`} aria-hidden="true"/>{tx(row.label)}</dt>
        <dd>{`~${formatTokens(breakdown[row.key], tx)}`}</dd>
      </div>)}
    </dl>
  </StatDialog>;
}

/** The composer dock: native whole-conversation totals and the last request's occupancy. */
export function ConversationStats({ statistics, occupancy }: { statistics?: ConversationStatistics | null; occupancy?: ContextOccupancy | null }) {
  const tx = useTranslation();
  if (!statistics && !occupancy) return null;
  return <div className={css.dock} aria-label={tx('agent:usage.session-title')} data-composer-dock="">
    {statistics && <ActivityPill statistics={statistics}/>}
    {statistics && <UsagePill statistics={statistics}/>}
    {occupancy && <ContextMeter occupancy={occupancy}/>}
  </div>;
}
