import { useMemo, useState } from 'react';
import type { RuntimeClientAgent } from '../../../../protocol/app-server/v80';
import { useTranslation } from '../../locale/react';
import { StateDot } from '../../presentation/primitives/StateDot';
import { Tooltip } from '../../presentation/primitives/Tooltip';
import { IconChevronDownOutline14, IconChevronRightOutline14, IconChevronUpOutline14 } from '../../presentation/primitives/icons';
import { useSubagents } from './subagent-context';
import { agentDot, agentRunning, agentStatus } from './subagent-state';
import css from './SessionPanelSubagents.module.css';

const FOLD_LIMIT = 5;

function sessionRootAgents(agents: readonly RuntimeClientAgent[], oneShot?: ReadonlySet<string>) {
  const visible = agents.filter(agent => !oneShot?.has(agent.agent_id));
  const ids = new Set(visible.map(agent => agent.agent_id));
  const roots = visible.filter(agent => !agent.parent_agent_id || !ids.has(agent.parent_agent_id));
  return [...roots].sort((left, right) => Number(agentRunning(right)) - Number(agentRunning(left)));
}

export function SubagentsRunningIndicator({ sessionId, panelShown }: { sessionId: string; panelShown: boolean }) {
  const tx = useTranslation();
  const scope = useSubagents();
  if (panelShown || !scope || scope.sessionId !== sessionId) return null;
  const running = scope.agents.filter(agent => !scope.oneShot?.has(agent.agent_id) && agentRunning(agent)).length;
  if (running === 0) return null;
  return (
    <span className={css.runningIndicator} data-subagents-running="">
      <span className={css.visuallyHidden}>{tx('common:subagents.count', { count: running })}</span>
    </span>
  );
}

export function SubagentsProgress({ sessionId }: { sessionId: string }) {
  const scope = useSubagents();
  if (!scope || scope.sessionId !== sessionId) return null;
  const roots = sessionRootAgents(scope.agents, scope.oneShot);
  if (roots.length === 0) return null;
  const done = roots.filter(agent => !agentRunning(agent)).length;
  return <span className={css.progress}>{`${done}/${roots.length}`}</span>;
}

export function SubagentsSection({ sessionId, onOpened }: { sessionId: string; onOpened?: () => void }) {
  const tx = useTranslation();
  const scope = useSubagents();
  const [unfolded, setUnfolded] = useState(false);
  const ordered = useMemo(
    () => (scope && scope.sessionId === sessionId ? sessionRootAgents(scope.agents, scope.oneShot) : []),
    [scope, sessionId],
  );
  if (!scope || scope.sessionId !== sessionId || ordered.length === 0) return null;
  const foldable = ordered.length > FOLD_LIMIT + 1;
  const shown = foldable && !unfolded ? ordered.slice(0, FOLD_LIMIT) : ordered;
  const remaining = ordered.length - FOLD_LIMIT;
  return (
    <div className={css.list} data-subagents-section="">
      {shown.map(agent => (
        <div key={agent.agent_id} data-session-panel-row="" data-agent-id={agent.agent_id}>
          <button
            type="button"
            data-row-part="body"
            aria-label={`${agent.title} ${agentStatus(agent, tx)}`}
            onClick={() => {
              scope.open(agent.agent_id);
              onOpened?.();
            }}
          >
            <span data-row-part="icon">
              <StateDot state={agentDot(agent)} size={7} />
            </span>
            <span data-row-part="content" className={css.rowTitle}>{agent.title}</span>
            <span className={css.rowStatus}>{agentStatus(agent, tx)}</span>
          </button>
          {scope.openAside && (
            <Tooltip label={tx('common:subagents.open-aside', { name: agent.title })} side="bottom" delayMs={500}>
              <button
                type="button"
                className={css.asideButton}
                aria-label={tx('common:subagents.open-aside', { name: agent.title })}
                onClick={event => {
                  event.stopPropagation();
                  scope.openAside?.(agent.agent_id);
                  onOpened?.();
                }}
              >
                <IconChevronRightOutline14 size={12} />
              </button>
            </Tooltip>
          )}
        </div>
      ))}
      {foldable && (
        <div data-session-panel-row="fold">
          <button
            type="button"
            data-row-part="body"
            aria-expanded={unfolded}
            onClick={() => setUnfolded(value => !value)}
          >
            <span data-row-part="icon">
              {unfolded ? <IconChevronUpOutline14 size={14} /> : <IconChevronDownOutline14 size={14} />}
            </span>
            <span data-row-part="content">
              {unfolded ? tx('common:activity.show-less') : tx('common:activity.show-more-count', { count: remaining })}
            </span>
          </button>
        </div>
      )}
    </div>
  );
}
