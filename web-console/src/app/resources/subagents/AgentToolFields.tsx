import { useState } from 'react';
import type { AgentProfileDocument } from '../../../../../protocol/app-server/v77';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { CheckboxList, Selection, nativeTools } from '../../settings/forms/controls';
import { Choice } from '../../settings/primitives/aria';
import css from './Subagents.module.css';

type Tools = NonNullable<AgentProfileDocument['tools']>;

export function AgentToolFields({ value, change }: { value?: Tools | null; change: (tools: Tools) => void }) {
  const tx = useTranslation();
  const [customizing, setCustomizing] = useState(false);
  const tools = value ?? {};
  const sources = Object.entries(tools.sources ?? {});
  const selected = !!tools.builtin?.length || sources.length > 0;
  const custom = customizing || selected;
  return <div className={css.tools}>
    <Choice label={tx('resources:subagents.tools')} value={custom ? 'selected' : 'none'}
      options={[['none', tx('resources:subagents.no-tools')], ['selected', tx('resources:subagents.selected-tools')]]}
      description={tx('resources:subagents.tools-help')}
      onChange={mode => {
        setCustomizing(mode === 'selected');
        if (mode === 'none' && selected) change({ builtin: [], sources: {} });
      }} />
    {custom && <div className={css.toolChoices}>
      <div className={css.builtins}>
        <CheckboxList label={tx('settings:extension-detail.native-tools')} values={nativeTools}
          selected={tools.builtin ?? []} change={builtin => change({ ...tools, builtin })} />
      </div>
      {sources.map(([id, selection]) => <div key={id}>
        <Selection label={id} value={selection} change={next => change({ ...tools, sources: { ...tools.sources, [id]: next } })} />
        <Button type="button" size="sm" onClick={() => {
          const next = { ...tools.sources };
          delete next[id];
          change({ ...tools, sources: next });
        }}>{tx('settings:extension-detail.remove-source-selection')} {id}</Button>
      </div>)}
    </div>}
  </div>;
}
