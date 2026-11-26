export const conversationPreferencesCss = {
  row: 'flex items-center gap-[8px] py-[16px] [border-bottom:0.5px_solid_var(--dsw-alias-border-l2)]',
  rowText: 'flex-1 min-w-0 flex flex-col gap-[4px] pr-[48px] max-[480px]:pr-[8px]',
  title: 'text-[14px] font-normal leading-[22px] [color:var(--dsw-alias-label-primary)]',
  desc: 'text-[12px] font-normal leading-[18px] [color:var(--dsw-alias-label-secondary)]',
  control: 'inline-flex items-center gap-[8px]',
  stepper: 'group/cpstep relative inline-flex items-center justify-center min-w-[72px] h-[36px] rounded-[var(--dsw-radius-md)] bg-[var(--dsw-alias-bg-module-platform)]',
  value: 'min-w-[18px] text-center text-[14px] leading-[22px] tabular-nums [color:var(--dsw-alias-label-primary)]',
  unit: 'text-[14px] leading-[22px] [color:var(--dsw-alias-label-secondary)]',
  arrows: 'absolute right-[8px] flex flex-col gap-[2px] opacity-0 group-focus-within/cpstep:opacity-100 [@media(hover:hover)]:group-hover/cpstep:opacity-100 [@media(hover:none)]:opacity-100',
  arrow: 'inline-flex items-center justify-center w-[17px] h-[12px] p-0 border-none rounded-[var(--dsw-radius-xs)] bg-[color-mix(in_srgb,var(--dsw-alias-bg-layer-1)_75%,transparent)] [color:var(--dsw-alias-label-primary)] cursor-pointer hover:not-disabled:bg-[var(--dsw-alias-bg-layer-1)] disabled:[color:var(--dsw-alias-label-caption)] disabled:cursor-default',
  selector: 'inline-flex items-center gap-[12px] h-[36px] px-[14px] border-none rounded-[var(--dsw-radius-md)] bg-[var(--dsw-alias-bg-module-platform)] [font:inherit] text-[14px] leading-[22px] [color:var(--dsw-alias-label-primary)] cursor-pointer hover:bg-[var(--dsw-alias-interactive-bg-hover)]',
  chevron: 'flex-none',
};
