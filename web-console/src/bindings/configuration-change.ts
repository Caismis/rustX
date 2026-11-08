import type { BoundedNames, ConfigurationChange, ConfigurationDomain, NamedChanges } from '../../../protocol/app-server/v77';
import type { Translate } from '../locale/translation';

/** Presentation of one recorded configuration commit. It reads only the
 * native change fact; current settings never explain a historical change. */
export function configurationDomainLabel(tx: Translate, domain: ConfigurationDomain): string {
  return tx(`trajectory:configuration.domain.${domain}`);
}

function counts(tx: Translate, changes: NamedChanges): string {
  return ([['added', changes.added], ['removed', changes.removed], ['changed', changes.changed]] as const)
    .filter(([, names]) => names.total > 0)
    .map(([kind, names]) => tx(`trajectory:configuration.${kind}`, { count: names.total }))
    .join(', ');
}

/** One line naming every changed domain, with Tool and Skill counts. */
export function configurationSummary(tx: Translate, change: ConfigurationChange): string {
  return change.domains.map(domain => {
    const label = configurationDomainLabel(tx, domain);
    const detail = domain === 'tools' ? counts(tx, change.tools) : domain === 'skills' ? counts(tx, change.skills) : '';
    return detail ? `${label} (${detail})` : label;
  }).join(' · ');
}

/** The recorded names, with the exact count the bounded list omits. */
export function configurationNames(tx: Translate, names: BoundedNames): string {
  const shown = names.names.join(', ');
  return names.total > names.names.length ? tx('trajectory:configuration.more', { names: shown, count: names.total - names.names.length }) : shown;
}
