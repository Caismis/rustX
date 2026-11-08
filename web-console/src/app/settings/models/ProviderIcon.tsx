import { useRef, useState } from 'react';
import { GridList, GridListItem } from 'react-aria-components';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { DialogSurface } from '../../../presentation/primitives/DialogSurface';
import { IconCloseOutline16 } from '../../../presentation/primitives/icons';
import { Search } from '../primitives/aria';
import { providerIcons } from './provider-icons.generated';
import css from './ProviderIcon.module.css';

/** Explicit provider metadata; names and API formats never choose a logo. */
export function ProviderIcon({ icon, size = 20 }: { icon?: string | null; size?: number }) {
  const asset = icon && Object.hasOwn(providerIcons, icon) ? providerIcons[icon] : undefined;
  return <span aria-hidden="true" className={css.icon} style={{ width: size, height: size }} data-provider-icon={icon ?? ''}>
    {asset ? asset.color ? <img src={asset.url} alt="" loading="lazy" width={size} height={size}/>
      : <span className={css.monochrome} style={{ maskImage: `url("${asset.url}")` }}/>
      : <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        {/* Lucide Package (ISC), from the project's pinned @iconify-json/lucide set. */}
        <path d="M11 21.73a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73zm1 .27V12"/>
        <path d="M3.29 7L12 12l8.71-5M7.5 4.27l9 5.15"/>
      </svg>}
  </span>;
}

export function ProviderIconField({ value, onChange }: { value?: string | null; onChange: (value: string | undefined) => void }) {
  const tx = useTranslation();
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLDivElement>(null);
  const label = value ? (Object.hasOwn(providerIcons, value) ? providerIcons[value].name : tx('settings:models.icon-unavailable')) : tx('settings:models.icon-default');
  return <div ref={trigger} className={css.field}>
    <span>{tx('settings:models.icon')}</span>
    <Button variant="outline" className={css.trigger} aria-label={tx('settings:models.choose-icon')}
      aria-haspopup="dialog" onClick={() => setOpen(true)}><ProviderIcon icon={value}/><span>{label}</span></Button>
    {open && <IconPicker value={value} onChange={onChange} onClose={() => setOpen(false)} finalFocus={() => trigger.current?.querySelector('button') ?? false}/>}
  </div>;
}

function IconPicker({ value, onChange, onClose, finalFocus }: {
  value?: string | null; onChange: (value: string | undefined) => void; onClose: () => void; finalFocus: () => HTMLElement | false;
}) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  const search = useRef<HTMLDivElement>(null);
  const filter = query.trim().toLocaleLowerCase();
  const icons = Object.entries(providerIcons).filter(([slug, asset]) => `${slug} ${asset.name}`.toLocaleLowerCase().includes(filter));
  const title = tx('settings:models.choose-icon');
  const select = (key: string) => { onChange(key === 'default' ? undefined : key); onClose(); };
  return <DialogSurface open title={title} onClose={onClose} overlayClassName={css.overlay} className={css.picker}
    initialFocus={() => search.current?.querySelector('input') ?? false} finalFocus={finalFocus}>
    <div className={css.header}><h2>{title}</h2><Button aria-label={tx('settings:models.close')} onClick={onClose}><IconCloseOutline16/></Button></div>
    <div ref={search}><Search label={tx('settings:models.search-icons')} placeholder={tx('settings:models.search-icons')} value={query} onChange={setQuery}/></div>
    <GridList aria-label={tx('settings:models.icon')} className={css.grid} layout="grid" selectionMode="single" selectedKeys={[value ?? 'default']}
      onSelectionChange={keys => { if (keys !== 'all') { const key = keys.values().next().value; if (key === undefined) onClose(); else select(String(key)); } }}>
      {!filter && <GridListItem id="default" textValue={tx('settings:models.icon-default')} className={css.option}>
        <ProviderIcon size={28}/><span>{tx('settings:models.icon-default')}</span>
      </GridListItem>}
      {icons.map(([slug, asset]) => <GridListItem id={slug} key={slug} textValue={asset.name} className={css.option}>
        <ProviderIcon icon={slug} size={28}/><span>{asset.name}</span>
      </GridListItem>)}
    </GridList>
    {!icons.length && <p className={css.empty}>{tx('settings:models.no-icons')}</p>}
    <a className={css.credit} href="https://icons.lobehub.com" target="_blank" rel="noreferrer">{/* i18n-raw: upstream project name */ 'LobeHub Icons'}</a>
  </DialogSurface>;
}
