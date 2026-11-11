import type {NativePolicyOverrideDocument} from '../../../../../protocol/app-server/v78';
import {useTranslation} from '../../../locale/react';
import {Choice} from '../primitives/aria';
export function PolicyFields({ value, change }: { value: NativePolicyOverrideDocument; change: (value: NativePolicyOverrideDocument) => void }) {
  const tx = useTranslation();
  return <>
    <Choice label={tx('settings:tools-page.execution')} value={value.execution ?? ''} onChange={execution => change({ ...value, execution: (execution || undefined) as NativePolicyOverrideDocument['execution'] })}
      options={[['', tx('settings:copy.domain-default')], ['foreground_only', tx('settings:policy.foreground_only')], ['background_only', tx('settings:policy.background_only')], ['model_selectable', tx('settings:policy.model_selectable')]]} />
  </>;
}
