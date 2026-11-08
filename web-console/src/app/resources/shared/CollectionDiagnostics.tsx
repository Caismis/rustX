import type {ResourceFamily,SourceSettings} from '../../../../../protocol/app-server/v77';
import {useTranslation} from '../../../locale/react';
import {Advanced} from '../../settings/primitives/aria';
import {extensionFamilyLabel} from '../../settings/projection';
import {collectionDiagnostics} from './inventory';
import css from '../../../presentation/settings/SettingsContent.module.css';
/** Diagnostics native attributes to a family's source document or collection
 * rather than to any one identity — a resource document that does not parse,
 * an unreadable directory, an exceeded catalog bound. They belong to the
 * family, so they are listed once here and never on the rows of the resources
 * those documents hold. Skill discovery reports its own typed diagnostics,
 * which name packages rather than Skill identities, so they are listed here on
 * the same terms. */
export function CollectionDiagnostics({ source, families }: { source: SourceSettings; families: readonly ResourceFamily[] }) {
  const tx = useTranslation();
  const diagnostics = collectionDiagnostics(source, families);
  const skills = families.includes('skill') ? (source.prospective_resources?.skill_diagnostics ?? []).filter(item => item.kind !== 'shadowed') : [];
  if (!diagnostics.length && !skills.length) return null;
  return <section aria-label={tx('settings:extensions-page.source-diagnostics')}>
    <h4>{tx('settings:extensions-page.source-diagnostics')}</h4>
    {diagnostics.map((item, index) => <p className={css.error} key={index}>
      {extensionFamilyLabel(tx, item.subject.family)} {tx('settings:extensions-page.source')}{item.file ? tx('settings:extensions-page.value', { p0: item.file }) : ''}: {item.reason}
    </p>)}
    {!!skills.length && <Advanced title={tx('settings:extensions-page.skill-discovery-diagnostics-value', { p0: skills.length })}>
      {skills.map((item, index) => <p className={css.error} key={index}>{tx(`settings:skill-issue.${item.kind}`)}{' '}{'package' in item ? item.package : 'root' in item ? item.root : 'name' in item ? item.name : item.roots.join(', ')}</p>)}
    </Advanced>}
  </section>;
}
