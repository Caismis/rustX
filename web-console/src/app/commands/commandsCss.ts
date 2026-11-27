export const commandsCss = {
  menu: 'absolute bottom-[calc(100%+4px)] left-0 right-0 z-100 p-[4px] max-h-[400px] overflow-y-auto overscroll-contain [scrollbar-width:thin] rounded-[var(--dsw-radius-xl)] bg-[var(--dsw-specific-menu)] [--dsw-elevation-stroke-color:var(--dsw-alias-border-l1)] shadow-[var(--dsw-elevation-prominent)]',
  menuRow: 'flex items-center gap-[6px] w-full min-h-[34px] px-[8px] py-[6px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-left cursor-pointer text-[13px] leading-[20px] aria-selected:bg-[var(--dsw-alias-interactive-bg-hover)]',
  sectionTitle: 'min-h-[23px] pt-[5px] px-[8px] pb-[2px] [color:var(--dsw-alias-label-tertiary)] text-[11px] font-medium leading-[16px] not-first:mt-[3px]',
  icon: 'inline-flex flex-none w-[14px] h-[14px] [color:var(--dsw-alias-label-tertiary)] [&_svg]:w-[14px] [&_svg]:h-[14px]',
  name: 'flex-none max-w-[40%] overflow-hidden text-ellipsis whitespace-nowrap',
  alias: 'flex-none max-w-[20%] overflow-hidden text-ellipsis whitespace-nowrap [color:var(--dsw-alias-label-tertiary)] text-[12px] leading-[18px]',
  descriptionCls: 'flex-1 min-w-0 text-right overflow-hidden text-ellipsis whitespace-nowrap [color:var(--dsw-alias-label-tertiary)] text-[12px] leading-[18px]',
  options: 'max-h-[40vh] overflow-y-auto',
  search: 'w-full mb-[8px]',
  row: 'flex items-center justify-between gap-[8px] w-full px-[10px] py-[8px] border-0 rounded-[var(--dsw-radius-sm)] bg-transparent [color:var(--dsw-alias-label-primary)] text-left cursor-pointer wrap-anywhere aria-selected:bg-[var(--dsw-alias-interactive-bg-hover)] hover:bg-[var(--dsw-alias-interactive-bg-hover)] [&_small]:[color:var(--dsw-alias-label-tertiary)]',
};
