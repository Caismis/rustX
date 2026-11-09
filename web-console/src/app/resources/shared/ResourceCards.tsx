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

export function ResourceCard({ name, label, icon, description, diagnostic, path, onOpen, availability, actions }: {
  name: string; label?: string; icon: ReactNode; description: string; diagnostic?: string;
  path: string; onOpen: () => void; availability?: ReactNode; actions?: ReactNode;
}) {
  return <li aria-label={name} className={clsx(catalog.card, diagnostic && catalog.invalidCard)}>
    <div className={clsx(catalog.cardHead, css.cardHeading)}>
      <button type="button" className={catalog.cardOpen} aria-label={label ?? name} onClick={onOpen}>
        <span className={catalog.fileTile}>{icon}</span>
        <span className={catalog.cardName}><span className={catalog.cardTitle}>{name}</span></span>
      </button>
      {availability && <div className={catalog.enable}>{availability}</div>}
    </div>
    <p className={clsx(catalog.cardDescription, css.description, diagnostic && catalog.invalid)}>{diagnostic ?? description}</p>
    <div className={clsx(catalog.cardFoot, css.cardFoot)}>
      <span className={catalog.path} title={path}><span>{path}</span></span>
      {actions && <div className={css.cardActions}>{actions}</div>}
    </div>
  </li>;
}
