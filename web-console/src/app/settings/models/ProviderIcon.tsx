import { useRef, useState } from 'react';
import { GridList, GridListItem } from 'react-aria-components';
import { useTranslation } from '../../../locale/react';
import { Button } from '../../../presentation/primitives/Button';
import { DialogSurface } from '../../../presentation/primitives/DialogSurface';
import { IconCloseOutline16 } from '../../../presentation/primitives/icons';
import { Search } from '../primitives/aria';
import { providerIcons } from './provider-icons.generated';

/** Explicit provider metadata; names and API formats never choose a logo. */
export function ProviderIcon({ icon, size = 20 }: { icon?: string | null; size?: number }) {
  const asset = icon && Object.hasOwn(providerIcons, icon) ? providerIcons[icon] : undefined;
  return <span aria-hidden="true" className="flex-none inline-flex items-center justify-center [color:var(--dsw-alias-label-primary)] [&_img]:block [&_img]:size-full [&_img]:object-contain" style={{ width: size, height: size }} data-provider-icon={icon ?? ''}>
    {asset ? asset.color ? <img src={asset.url} alt="" loading="lazy" width={size} height={size}/>
      : <span className="block size-full bg-current mask-contain mask-center mask-no-repeat" style={{ maskImage: `url("${asset.url}")` }}/>
      : <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round">
        {/* Lucide Package (ISC), from the project's pinned @iconify-json/lucide set. */}
        <path d="M11 21.73a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73zm1 .27V12"/>
        <path d="M3.29 7L12 12l8.71-5M7.5 4.27l9 5.15"/>
      </svg>}
  </span>;
}

export function IconPicker({ value, onChange, onClose, finalFocus }: {
  value?: string | null; onChange: (value: string | undefined) => void; onClose: () => void; finalFocus: () => HTMLElement | false;
}) {
  const tx = useTranslation();
  const [query, setQuery] = useState('');
  const search = useRef<HTMLDivElement>(null);
  const filter = query.trim().toLocaleLowerCase();
  const icons = Object.entries(providerIcons).filter(([slug, asset]) => `${slug} ${asset.name}`.toLocaleLowerCase().includes(filter));
  const title = tx('settings:models.choose-icon');
  const select = (key: string) => { onChange(key === 'default' ? undefined : key); onClose(); };
  return <DialogSurface open title={title} onClose={onClose} overlayClassName="fixed inset-0 z-[1250] flex items-center justify-center p-[16px] box-border bg-[var(--dsw-alias-bg-mask-1)] [backdrop-filter:var(--dsw-mask-blur)]" className="w-[min(576px,100%)] max-h-[min(640px,calc(100dvh-32px))] flex flex-col gap-[16px] box-border p-[20px] max-[480px]:p-[16px] [color:var(--dsw-alias-label-primary)] bg-[var(--dsw-alias-bg-layer-2)] border-0 rounded-[var(--dsw-radius-lg)] shadow-[var(--dsw-elevation-prominent)] outline-none"
    initialFocus={() => search.current?.querySelector('input') ?? false} finalFocus={finalFocus}>
    <div className="flex justify-between items-center gap-[16px] [&_h2]:m-0 [&_h2]:text-[18px] [&_h2]:font-medium [&_button]:p-0 [&_button]:size-[28px]"><h2>{title}</h2><Button aria-label={tx('settings:models.close')} onClick={onClose}><IconCloseOutline16/></Button></div>
    <div ref={search}><Search label={tx('settings:models.search-icons')} placeholder={tx('settings:models.search-icons')} value={query} onChange={setQuery}/></div>
    <GridList aria-label={tx('settings:models.icon')} className="min-h-[100px] grid grid-cols-[repeat(auto-fill,minmax(88px,1fr))] gap-[8px] overflow-auto overscroll-contain p-[3px] m-[-3px] list-none" layout="grid" selectionMode="single" selectedKeys={[value ?? 'default']}
      onSelectionChange={keys => { if (keys !== 'all') { const key = keys.values().next().value; if (key === undefined) onClose(); else select(String(key)); } }}>
      {!filter && <GridListItem id="default" textValue={tx('settings:models.icon-default')} className="flex flex-col items-center justify-center gap-[10px] min-w-0 min-h-[82px] px-[4px] py-[8px] box-border border border-transparent rounded-[var(--dsw-radius-md)] text-[12px] cursor-pointer outline-none data-[hovered]:bg-[var(--dsw-alias-interactive-bg-hover)] data-[hovered]:[border-color:var(--dsw-alias-border-l4)] data-[selected]:bg-[var(--dsw-alias-interactive-bg-hover)] data-[selected]:[border-color:var(--dsw-alias-border-l4)] data-[focus-visible]:outline-2 data-[focus-visible]:outline-[var(--dsw-alias-label-primary)] data-[focus-visible]:outline-offset-1 [&_span:last-child]:text-center [&_span:last-child]:wrap-anywhere">
        <ProviderIcon size={28}/><span>{tx('settings:models.icon-default')}</span>
      </GridListItem>}
      {icons.map(([slug, asset]) => <GridListItem id={slug} key={slug} textValue={asset.name} className="flex flex-col items-center justify-center gap-[10px] min-w-0 min-h-[82px] px-[4px] py-[8px] box-border border border-transparent rounded-[var(--dsw-radius-md)] text-[12px] cursor-pointer outline-none data-[hovered]:bg-[var(--dsw-alias-interactive-bg-hover)] data-[hovered]:[border-color:var(--dsw-alias-border-l4)] data-[selected]:bg-[var(--dsw-alias-interactive-bg-hover)] data-[selected]:[border-color:var(--dsw-alias-border-l4)] data-[focus-visible]:outline-2 data-[focus-visible]:outline-[var(--dsw-alias-label-primary)] data-[focus-visible]:outline-offset-1 [&_span:last-child]:text-center [&_span:last-child]:wrap-anywhere">
        <ProviderIcon icon={slug} size={28}/><span>{asset.name}</span>
      </GridListItem>)}
    </GridList>
    {!icons.length && <p className="text-center text-[13px] [color:var(--dsw-alias-label-secondary)]">{tx('settings:models.no-icons')}</p>}
    <a className="self-end text-[11px] [color:var(--dsw-alias-label-secondary)]" href="https://icons.lobehub.com" target="_blank" rel="noreferrer">{/* i18n-raw: upstream project name */ 'LobeHub Icons'}</a>
  </DialogSurface>;
}
