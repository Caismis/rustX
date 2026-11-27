export const modalCss = {
  root: 'fixed inset-0 z-[1000] flex items-center justify-center p-[24px]',
  mask: 'overscroll-contain absolute inset-0 bg-[var(--dsw-alias-bg-mask-1)] [backdrop-filter:var(--dsw-mask-blur)]',
  dialog: 'overscroll-contain relative z-1 flex flex-col gap-[20px] w-[min(380px,100%)] px-0 pb-[24px] overflow-hidden border-0 rounded-[24px] bg-[var(--dsw-alias-bg-layer-2)] shadow-[var(--dsw-elevation-prominent)] max-h-[calc(100dvh-48px)]',
  content: 'flex flex-col w-full min-h-0 overflow-y-auto',
  header: 'flex flex-shrink-0 items-center justify-between gap-[8px] pt-[22px] pr-[14px] pb-[12px] pl-[24px]',
  titleCls: 'm-0 text-[16px] leading-[24px] font-medium [color:var(--dsw-alias-label-primary)]',
  close: 'flex-none inline-flex items-center justify-center w-[28px] h-[28px] border-none rounded-[8px] bg-transparent cursor-pointer [color:var(--dsw-alias-label-secondary)] hover:bg-[var(--dsw-alias-interactive-bg-hover)]',
  descriptionCls: 'm-0 px-[24px] text-[14px] leading-[22px] font-normal [color:var(--dsw-alias-label-primary)]',
  body: 'flex flex-col min-w-0 mt-[20px] px-[24px] wrap-anywhere',
  footer: 'flex flex-shrink-0 items-center justify-end gap-[8px] px-[24px]',
  scrim: 'bg-[var(--dsw-alias-bg-mask-1)] [backdrop-filter:var(--dsw-mask-blur)]',
};
