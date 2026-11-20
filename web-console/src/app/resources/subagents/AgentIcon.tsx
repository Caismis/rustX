import { BundledIcon } from '../../../presentation/primitives/BundledIcon';
import { IconAgentPresetOutline16 } from '../../../presentation/primitives/icons';

export function AgentIcon({ icon, size = 16 }: { icon?: string | null; size?: number }) {
  const fallback = <IconAgentPresetOutline16 size={size} />;
  return icon ? <BundledIcon name={icon} size={size} fallback={fallback} /> : fallback;
}
