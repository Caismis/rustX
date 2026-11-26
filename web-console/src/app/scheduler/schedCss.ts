/* Copyright (c) 2026 DeepSeek. MIT. Source-derived; see PROVENANCE.md. */
/* Tailwind utility strings replacing scheduler.module.css — shared across scheduler surfaces. */
export const schedCss = {
  loading: '[color:var(--dsw-alias-label-tertiary)] text-[12px]',
  instructionLabel: 'block [color:var(--dsw-alias-label-secondary)] text-[12px] mb-[8px]',
  inlineActions: 'flex gap-[8px] flex-wrap justify-end',
  sectionTitle: 'text-[13px] font-medium mt-[16px] mb-[8px]',
  result: 'whitespace-pre-wrap wrap-anywhere max-h-[280px] overflow-auto p-[12px] bg-[var(--dsw-alias-bg-module-platform)] rounded-[var(--dsw-radius-sm)] text-[12px]',
  savedVersion: 'whitespace-pre-wrap wrap-anywhere max-h-[140px] overflow-auto p-[12px] bg-[var(--dsw-alias-bg-module-platform)] rounded-[var(--dsw-radius-sm)] text-[12px]',
  technical: '[color:var(--dsw-alias-label-tertiary)] text-[12px] py-[16px] wrap-anywhere [&_summary]:cursor-pointer [&_dl]:grid [&_dl]:grid-cols-[max-content_minmax(0,1fr)] [&_dl]:gap-[8px] [&_dd]:m-0 [&_dd]:select-text',
  steps: 'list-none m-0 p-0 [&_li]:flex [&_li]:justify-between [&_li]:gap-[16px] [&_li]:py-[8px] [&_li]:[border-bottom:0.5px_solid_var(--dsw-alias-border-l4)] [&_li_span:last-child]:[color:var(--dsw-alias-label-secondary)]',
  zoneCombo: 'flex-[0_1_68%] min-w-0',
  pickerButton: 'border-0 bg-transparent [color:inherit] [font:inherit] text-[13px] px-[8px] py-[6px] rounded-[var(--dsw-radius-xl)] hover:not-disabled:bg-[var(--dsw-alias-interactive-bg-hover)]',
  zoneInput: 'flex items-center [&_input]:w-full [&_input]:min-w-0 [&_input]:border-0 [&_input]:bg-transparent [&_input]:[color:inherit] [&_input]:[font:inherit] [&_input]:text-[13px] [&_input]:text-right [&_input]:outline-offset-3 [&_input]:[@media(pointer:coarse)]:text-[16px] [&_button]:flex [&_button]:p-[4px] [&_button]:border-0 [&_button]:[color:inherit] [&_button]:bg-transparent',
  zonePopup: 'z-[1500] w-[min(420px,calc(100vw-32px))] max-h-[320px] overflow-auto p-[4px] rounded-[var(--dsw-radius-md)] bg-[var(--dsw-specific-menu)] [color:var(--dsw-alias-label-primary)] shadow-[var(--dsw-elevation-prominent)]',
  zoneList: 'outline-none',
  zoneOption: 'px-[12px] py-[7px] rounded-[var(--dsw-radius-sm)] text-[13px] cursor-pointer outline-none data-[focused]:bg-[var(--dsw-alias-interactive-bg-hover)] data-[selected]:bg-[var(--dsw-alias-interactive-bg-hover)]',
  taskTab: 'flex min-h-0 h-full [&>aside]:flex-1 [&>aside]:border-0',
  clockMark: 'inline-flex items-center justify-center [color:var(--dsw-alias-label-tertiary)]',
  hoverTasks: '[border-top:0.5px_solid_var(--dsw-alias-border-l4)] mt-[8px] pt-[8px] text-[12px] [&_p]:mt-[6px] [&_p]:mb-0 [&_small]:block [&_small]:[color:var(--dsw-alias-label-tertiary)]',
  created: 'flex gap-[12px] items-center [border:0.5px_solid_var(--dsw-alias-border-l2)] rounded-[var(--dsw-radius-md)] px-[14px] py-[12px] max-w-[520px] my-[12px] [&>div]:flex-1 [&>div]:min-w-0 [&_strong]:block [&_strong]:wrap-anywhere [&_small]:block [&_small]:wrap-anywhere [&_small]:[color:var(--dsw-alias-label-secondary)] [&_small]:mt-[3px]',
};
