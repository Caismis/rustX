import type { ReactNode } from 'react';
import clsx from 'clsx';
import { catalogCss } from '../../../presentation/catalog/catalogCss';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import { resourceCatalogCss } from './resourceCatalogCss';

export function ResourceGroup({ label, count, view, empty, children }: {
  label: string; count: number; view: CatalogView; empty: ReactNode; children: ReactNode;
}) {
  return <section className={resourceCatalogCss.group} role="group" aria-label={label}>
    <h2 className={clsx(catalogCss.context, resourceCatalogCss.groupTitle)}>{label}<span aria-hidden> · </span>{count}</h2>
    {count ? <ul role="list" className={clsx(catalogCss.cards, view === 'list' && resourceCatalogCss.list)}>{children}</ul> : <div className={catalogCss.state}>{empty}</div>}
  </section>;
}

export function ResourceCard({ name, subtitle, label, icon, description, diagnostic, onOpen, availability, actions, menu }: {
  name: string; subtitle?: string; label?: string; icon: ReactNode; description: string; diagnostic?: string;
  onOpen: () => void; availability?: ReactNode; actions?: ReactNode; menu?: ReactNode;
}) {
  return <li aria-label={name} className={clsx(catalogCss.card, catalogCss.fixedCard, resourceCatalogCss.resourceCard, diagnostic && catalogCss.invalidCard)}>
    <div className={clsx(catalogCss.cardHead, resourceCatalogCss.cardHeading)}>
      <button type="button" className={catalogCss.cardOpen} aria-label={label ?? name} onClick={onOpen}>
        <span className={catalogCss.fileTile}>{icon}</span>
        <span className={catalogCss.cardName}>
          <span className={catalogCss.cardTitle}>{name}</span>
          {subtitle && <span className={catalogCss.cardId}>{subtitle}</span>}
        </span>
      </button>
      {(availability || menu) && <div className={resourceCatalogCss.headControls}>
        {availability && <div className={catalogCss.enable}>{availability}</div>}
        {menu}
      </div>}
    </div>
    {(diagnostic || description) && <p className={clsx(catalogCss.cardDescription, resourceCatalogCss.description, diagnostic && catalogCss.invalid)}>{diagnostic ?? description}</p>}
    {actions && <div className={resourceCatalogCss.cardActions}>{actions}</div>}
  </li>;
}
