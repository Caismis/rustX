import type { ReactNode } from 'react';
import clsx from 'clsx';
import type { CatalogView } from '../../../presentation/catalog/CatalogControls';
import catalog from '../../../presentation/catalog/Catalog.module.css';
import css from './ResourceCatalog.module.css';

export function ResourceGroup({ label, count, view, empty, children }: {
  label: string; count: number; view: CatalogView; empty: ReactNode; children: ReactNode;
}) {
  return <section className={css.group} role="group" aria-label={label}>
    <h2 className={clsx(catalog.context, css.groupTitle)}>{label}<span aria-hidden> · </span>{count}</h2>
    {count ? <ul role="list" className={clsx(catalog.cards, view === 'list' && css.list)}>{children}</ul> : <div className={catalog.state}>{empty}</div>}
  </section>;
}

export function ResourceCard({ name, subtitle, label, icon, description, diagnostic, onOpen, availability, actions, menu }: {
  name: string; subtitle?: string; label?: string; icon: ReactNode; description: string; diagnostic?: string;
  onOpen: () => void; availability?: ReactNode; actions?: ReactNode; menu?: ReactNode;
}) {
  return <li aria-label={name} className={clsx(catalog.card, css.resourceCard, diagnostic && catalog.invalidCard)}>
    <div className={clsx(catalog.cardHead, css.cardHeading)}>
      <button type="button" className={catalog.cardOpen} aria-label={label ?? name} onClick={onOpen}>
        <span className={catalog.fileTile}>{icon}</span>
        <span className={catalog.cardName}>
          <span className={catalog.cardTitle}>{name}</span>
          {subtitle && <span className={catalog.cardId}>{subtitle}</span>}
        </span>
      </button>
      {availability && <div className={catalog.enable}>{availability}</div>}
      {menu}
    </div>
    {(diagnostic || description) && <p className={clsx(catalog.cardDescription, css.description, diagnostic && catalog.invalid)}>{diagnostic ?? description}</p>}
    {actions && <div className={css.cardActions}>{actions}</div>}
  </li>;
}
