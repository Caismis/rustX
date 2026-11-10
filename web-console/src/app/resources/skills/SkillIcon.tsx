import { useState } from 'react';
import type { SkillDisplayIcon } from '../../../../../protocol/app-server/v77';
import { BundledIcon } from '../../../presentation/primitives/BundledIcon';
import { IconSkillOutline16 } from '../../../presentation/primitives/icons';

export function SkillIcon({ icon, size = 18 }: { icon?: SkillDisplayIcon | null; size?: number }) {
  const [failed, setFailed] = useState<string>();
  const fallback = <IconSkillOutline16 size={size} />;
  if (icon?.kind === 'iconify') return <BundledIcon name={icon.name} size={size} fallback={fallback} />;
  if (icon?.kind === 'asset') {
    const src = `data:${icon.media_type};base64,${icon.data_base64}`;
    // Keep SVG assets in the browser's image context.
    if (failed !== src) return <img src={src} alt="" width={size} height={size}
      style={{ objectFit: 'contain', flexShrink: 0 }} onError={() => setFailed(src)} />;
  }
  return fallback;
}
