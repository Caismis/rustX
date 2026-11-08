import type {NativePolicyOverrideDocument} from '../../../../../protocol/app-server/v77';
import {useTranslation} from '../../../locale/react';
import {Choice} from '../primitives/aria';
export function PolicyFields({ value, change }: { value: NativePolicyOverrideDocument; change: (value: NativePolicyOverrideDocument) => void }) {
  const tx = useTranslation();
  return <>
    <Choice label={tx('settings:tools-page.execution')} value={value.execution ?? ''} onChange={execution => change({ ...value, execution: (execution || undefined) as NativePolicyOverrideDocument['execution'] })}
      options={[['', tx('settings:copy.domain-default')], ['foreground_only', tx('settings:policy.foreground_only')], ['background_only', tx('settings:policy.background_only')], ['model_selectable', tx('settings:policy.model_selectable')]]} />
    <Choice label={tx('settings:tools-page.concurrency')} value={value.concurrency ?? ''} onChange={concurrency => change({ ...value, concurrency: (concurrency || undefined) as NativePolicyOverrideDocument['concurrency'] })}
      options={[['', tx('settings:copy.domain-default')], ['sequential', tx('settings:policy.sequential')], ['parallel', tx('settings:policy.parallel')]]} />
    <Choice label={tx('settings:tools-page.approval-2')} value={value.approval ?? ''} onChange={approval => change({ ...value, approval: (approval || undefined) as NativePolicyOverrideDocument['approval'] })}
      options={[['', tx('settings:copy.domain-default')], ['never', tx('settings:policy.never')], ['always', tx('settings:policy.always')]]} />
  </>;
}
