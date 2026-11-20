import type { ComponentProps } from 'react';
import { BundledIcon } from '../../../presentation/primitives/BundledIcon';
import { IconPicker } from '../../../presentation/primitives/IconPicker';
import { IconWorkflow } from './icons';

/** The icon a Workflow shows when its display names none. */
export const DEFAULT_WORKFLOW_ICON = 'lucide:workflow';

/** A Workflow's chosen icon; the Workflow glyph until a bundled set has it. */
export function WorkflowIcon({ icon, size = 18, className }: { icon?: string | null; size?: number; className?: string }) {
  return <BundledIcon name={icon ?? DEFAULT_WORKFLOW_ICON} size={size} className={className}
    fallback={<IconWorkflow size={size} className={className} />} />;
}

export function WorkflowIconPicker(props: Omit<ComponentProps<typeof IconPicker>, 'children'>) {
  return <IconPicker {...props}><WorkflowIcon icon={props.value} size={22} /></IconPicker>;
}
