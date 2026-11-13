import type { ResourcePermissionSet } from '../../../../../protocol/app-server/v79';
import { useTranslation } from '../../../locale/react';
import { Choice } from '../primitives/aria';

const VALUES = ['default', 'allow', 'ask', 'deny'] as const;
type PermissionValue = (typeof VALUES)[number];

const BLANK: ResourcePermissionSet = { plan: 'default', automatic: 'default', full_access: 'default' };

/** Complete three-mode resource permission object (Issue #544). */
export function ResourcePermissionFields({
  value,
  change,
}: {
  value: ResourcePermissionSet | null | undefined;
  change: (value: ResourcePermissionSet | null) => void;
}) {
  const tx = useTranslation();
  const current = value ?? BLANK;
  const options = VALUES.map(v => [v, tx(`settings:permission.${v}`)] as [string, string]);
  const set = (mode: keyof ResourcePermissionSet, next: PermissionValue) => {
    const updated = { ...current, [mode]: next };
    const allDefault = updated.plan === 'default' && updated.automatic === 'default' && updated.full_access === 'default';
    change(allDefault && value == null ? null : updated);
  };
  return (
    <>
      <Choice
        label={tx('settings:permission.plan')}
        value={current.plan}
        onChange={next => set('plan', next as PermissionValue)}
        options={options}
      />
      <Choice
        label={tx('settings:permission.automatic')}
        value={current.automatic}
        onChange={next => set('automatic', next as PermissionValue)}
        options={options}
      />
      <Choice
        label={tx('settings:permission.full-access')}
        value={current.full_access}
        onChange={next => set('full_access', next as PermissionValue)}
        options={options}
      />
    </>
  );
}

export { BLANK as blankResourcePermissions };
