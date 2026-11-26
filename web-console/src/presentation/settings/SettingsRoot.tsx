import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useCallback, useEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { Tabs, TabsContent, TabsList, TabsTab } from '@/components/ui/tabs';
import clsx from 'clsx';
import { Menu } from '../primitives/Menu';
import { IconChevronDownOutline14 } from '../primitives/icons';
import { settingsWorkflowCss } from './settingsWorkflowCss';

/** One primary Settings page as the panel presents it. The icon is a
 * recognition aid only: the label is always the page's accessible name. */
export interface SettingsPageEntry { id: string; label: string; icon: ReactNode }

/** A first-class Settings page with internal section navigation. Container
 * queries choose the section tabs or compact menu; neither owns Shell routing.
 * Native configuration actors outlive this retained presentation. */
export function SettingsPanel({ pages, activeId, onSelect, visible = true, children }: {
  /** `activeId` is always one of `pages`: the owner of navigation admits no
   * other, so it is rendered as it is. */
  pages: readonly SettingsPageEntry[]; activeId: string; onSelect: (id: string) => void;
  visible?: boolean; children: ReactNode;
}) {
  const tx = useTranslation();
  const [sections, setSections] = useState(false);
  const panelRef = useRef<HTMLElement>(null);
  const [panelNode, setPanelNode] = useState<HTMLElement | null>(null);
  const attachPanel = useCallback((node: HTMLElement | null) => {
    panelRef.current = node;
    setPanelNode(node);
  }, []);
  const sectionTriggerRef = useRef<HTMLButtonElement>(null);
  useNavigationFocusHandoff(panelNode, sectionTriggerRef);
  useEffect(() => { if (!visible) setSections(false); }, [visible]);
  const active = pages.find(page => page.id === activeId)!;
  return (
    <section ref={attachPanel} className="flex-1 min-w-0 min-h-0 h-full flex overflow-hidden bg-[var(--dsw-alias-bg-base)] [container:settings/inline-size]" aria-label={tx('settings:settings.settings')} tabIndex={-1} data-settings-page>
      <div className="flex-1 min-w-0 min-h-0 grid grid-cols-[216px_minmax(0,1fr)] grid-rows-[auto_minmax(0,1fr)] [grid-template-areas:'nav_header'_'nav_options'] w-full outline-none [@container_settings_(max-width:680px)]:grid-cols-[minmax(0,1fr)] [@container_settings_(max-width:680px)]:[grid-template-areas:'header'_'options']">
          <header data-settings-header="" className="[grid-area:header] hidden grid-cols-[minmax(0,1fr)] items-center gap-x-[12px] gap-y-[8px] min-h-[60px] pt-[14px] pr-[16px] pb-[10px] pl-[28px] box-border [@container_settings_(max-width:680px)]:grid [@container_settings_(max-width:680px)]:grid-cols-[minmax(0,1fr)] [@container_settings_(max-width:680px)]:pt-[12px] [@container_settings_(max-width:680px)]:pr-[12px] [@container_settings_(max-width:680px)]:pb-[10px] [@container_settings_(max-width:680px)]:pl-[16px]">
            <div className="min-w-0 [@container_settings_(max-width:680px)]:block">
              <Menu open={sections} onClose={() => setSections(false)} autoFocus focusOwner={panelRef}
                items={pages.map(page => ({ id: page.id, label: page.label, icon: page.icon }))} selectedId={activeId}
                onSelect={id => { setSections(false); onSelect(id); }}
                anchor={<button ref={sectionTriggerRef} type="button" data-settings-section-trigger="" className="flex items-center gap-[8px] max-w-full h-[40px] py-[9px] px-[12px] box-border [border:0.5px_solid_var(--dsw-alias-border-l3)] rounded-[12px] bg-transparent cursor-pointer [font-family:inherit] text-[14px] leading-[22px] font-medium [color:var(--dsw-alias-label-primary)] hover:bg-[var(--dsw-alias-interactive-bg-hover)] focus-visible:[outline:2px_solid_var(--dsw-alias-state-business-primary)] focus-visible:outline-offset-2" aria-haspopup="menu" aria-expanded={sections}
                  aria-label={tx('settings:settings-root.settings-page-value', { p0: active.label })} onClick={() => setSections(open => !open)}>
                  <span className="flex-none inline-flex w-[16px] h-[16px] items-center justify-center [color:var(--dsw-alias-label-secondary)]" aria-hidden="true">{active.icon}</span>
                  <span className="min-w-0 overflow-hidden whitespace-nowrap text-ellipsis">{active.label}</span>
                  <IconChevronDownOutline14 className="flex-none [color:var(--dsw-alias-label-tertiary)]" />
                </button>} />
            </div>
          </header>
          <Tabs className="contents" orientation="vertical" value={activeId} onValueChange={key => { onSelect(String(key)); }}>
            <nav className="[border-right:1px_solid_var(--dsw-alias-border-l3)] flex-none flex flex-col gap-[18px] [grid-area:nav] min-h-0 pt-[22px] px-[12px] pb-0 box-border [@container_settings_(max-width:680px)]:hidden" aria-label={tx('settings:navigation.rail')} data-settings-navigation="">
              <div className="px-[12px] py-0 text-[16px] leading-[24px] font-medium [color:var(--dsw-alias-label-primary)]">{tx('settings:settings.settings')}</div>
              <TabsList activateOnFocus className={clsx('flex flex-col gap-[4px] overflow-y-auto min-h-0 pb-[20px]', settingsWorkflowCss.pageTabs)} aria-label={tx('settings:settings-root.settings-pages')}>
                {pages.map(page => (
                  <TabsTab key={page.id} value={page.id} className={clsx('flex items-center gap-[8px] h-[40px] pt-[9px] pr-[16px] pb-[9px] pl-[12px] box-border border-none rounded-[12px] bg-transparent cursor-pointer [font-family:inherit] text-[14px] leading-[22px] font-normal [color:var(--dsw-alias-label-primary)] text-left', settingsWorkflowCss.pageTab)}>
                    <span className="flex-none inline-flex w-[16px] h-[16px] items-center justify-center [color:var(--dsw-alias-label-secondary)]" aria-hidden="true">{page.icon}</span>
                    <span className={clsx('flex-1 min-w-0 overflow-hidden whitespace-nowrap text-ellipsis', settingsWorkflowCss.pageTabLabel)}>{page.label}</span>
                  </TabsTab>
                ))}
              </TabsList>
            </nav>
            <div className="[grid-area:options] min-w-0 min-h-0 flex overflow-hidden [@container_settings_(max-width:680px)]:mb-0 [&_[role=tabpanel]]:flex-1 [&_[role=tabpanel]]:min-w-0 [&_[role=tabpanel]]:min-h-0 [&_[role=tabpanel]]:flex [&_[role=tabpanel]]:flex-col [&_[role=tabpanel]]:overflow-hidden">
              <TabsContent key={activeId} value={activeId} className={settingsWorkflowCss.pageTabPanel}>{children}</TabsContent>
            </div>
          </Tabs>
      </div>
    </section>
  );
}

/** A page tab on the rail — not a tab list inside a page's content. */
const RAIL_TAB = 'nav[data-settings-navigation] [role="tab"]';

/**
 * Keyboard continuity across the panel's two navigation presentations.
 *
 * The container query alone decides which presentation is rendered. When it
 * takes the one holding the keyboard out of layout, the browser's focus fixup
 * blurs that control: `focusout` with no `relatedTarget`, on a control that no
 * longer renders. That report — the focused navigation control is gone — is
 * the only trigger; nothing here measures the panel or knows the 680px rule,
 * so there is no second responsive authority to disagree with the CSS.
 *
 * A hidden rail tab hands the keyboard to the section trigger, and a hidden
 * section trigger (its menu closed; an open menu settles on its own focus
 * owner) to the selected rail tab: the same page, in the presentation that is
 * now shown, focused without scrolling. The handoff runs inside the fixup's
 * own `focusout`, so the keyboard is back on a Settings control before
 * anything renders or React Aria's containment looks for it. A counterpart
 * that is not rendered refuses focus, and the panel — where a freshly opened
 * Settings starts — takes it instead, so the handoff never lands on a hidden
 * control and never bounces between the two. Selection, the section menu,
 * drafts and native state are not touched.
 */
function useNavigationFocusHandoff(panel: HTMLElement | null, sectionTriggerRef: RefObject<HTMLButtonElement | null>): void {
  useEffect(() => {
    if (panel === null) return;
    const onFocusOut = (event: FocusEvent) => {
      const control = event.target;
      // A focus move to somewhere, a window blur, or a removal is not this.
      if (event.relatedTarget !== null || !(control instanceof HTMLElement) || !control.isConnected || control.checkVisibility()) return;
      const counterpart = control === sectionTriggerRef.current
        ? panel.querySelector<HTMLElement>(`${RAIL_TAB}[aria-selected="true"]`)
        : control.matches(RAIL_TAB) ? sectionTriggerRef.current : undefined;
      if (counterpart === undefined) return;
      for (const owner of [counterpart, panel]) {
        owner?.focus({ preventScroll: true });
        if (document.activeElement === owner) return;
      }
    };
    panel.addEventListener('focusout', onFocusOut);
    return () => { panel.removeEventListener('focusout', onFocusOut); };
  }, [panel, sectionTriggerRef]);
}
