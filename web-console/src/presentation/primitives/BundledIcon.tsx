import { useEffect, useState, type ReactNode } from 'react';
import { Icon, addCollection, type IconifyJSON } from '@iconify/react/offline';

// The Offline build never requests a remote Iconify set.
let lucide: Promise<IconifyJSON> | undefined;

export function useBundledIcons() {
  const [set, setSet] = useState<IconifyJSON>();
  useEffect(() => {
    let live = true;
    lucide ??= import('@iconify-json/lucide/icons.json').then(module => {
      const collection = module.default as IconifyJSON;
      addCollection(collection);
      return collection;
    });
    void lucide.then(value => { if (live) setSet(value); });
    return () => { live = false; };
  }, []);
  return set;
}

export function hasBundledIcon(set: IconifyJSON | undefined, icon: string) {
  const [prefix, name] = icon.split(':');
  return !!set && prefix === set.prefix && !!name && (Object.hasOwn(set.icons, name) || Object.hasOwn(set.aliases ?? {}, name));
}

export function BundledIcon({ name, size, className, fallback }: {
  name: string; size: number; className?: string; fallback: ReactNode;
}) {
  return hasBundledIcon(useBundledIcons(), name)
    ? <Icon icon={name} width={size} height={size} className={className} aria-hidden="true" />
    : fallback;
}
