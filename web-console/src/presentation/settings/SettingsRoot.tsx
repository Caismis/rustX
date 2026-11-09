import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useCallback, useEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { Tab, TabList, TabPanel, Tabs } from 'react-aria-components';
import clsx from 'clsx';
import { Menu } from '../primitives/Menu';
import { IconChevronDownOutline14 } from '../primitives/icons';
import css from './SettingsRoot.module.css';
import workflow from './SettingsWorkflow.module.css';

/** One primary Settings page as the panel presents it. The icon is a
 * recognition aid only: the label is always the page's accessible name. */
export interface SettingsPageEntry { id: string; label: string; icon: ReactNode }

/** A first-class Settings page with internal section navigation. Container
 * queries choose the section tabs or compact menu; neither owns Shell routing.
 * Native configuration actors outlive this retained presentation. */
export function SettingsPanel({ pages, activeId, onSelect, visible = true, context, children }: {
  /** `activeId` is always one of `pages`: the owner of navigation admits no
   * other, so it is rendered as it is. */
  pages: readonly SettingsPageEntry[]; activeId: string; onSelect: (id: string) => void;
  /** The owner and observation state of the open Settings instance. It sits in
   * the fixed header, so every page shares it and none of it scrolls away. */
  context?: ReactNode;
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
    <section ref={attachPanel} className={css.panel} aria-label={tx('settings:settings.settings')} tabIndex={-1} data-settings-page>
      <div className={css.page}>
          {/* The header is deliberately outside <Tabs>: React Aria renders a
              Tabs subtree a second time into a detached collection document
              to discover its tabs, and the section menu measures real DOM. */}
          <header data-settings-header="" className={css.header}>
            <div className={css.sections}>
              <Menu open={sections} onClose={() => setSections(false)} autoFocus focusOwner={panelRef}
                items={pages.map(page => ({ id: page.id, label: page.label, icon: page.icon }))} selectedId={activeId}
                onSelect={id => { setSections(false); onSelect(id); }}
                anchor={<button ref={sectionTriggerRef} type="button" className={css.sectionTrigger} aria-haspopup="menu" aria-expanded={sections}
                  aria-label={tx('settings:settings-root.settings-page-value', { p0: active.label })} onClick={() => setSections(open => !open)}>
                  <span className={css.navIcon} aria-hidden="true">{active.icon}</span>
                  <span className={css.sectionLabel}>{active.label}</span>
                  <IconChevronDownOutline14 className={css.sectionChevron} />
                </button>} />
            </div>
            <div className={css.context}>{context}</div>
          </header>
          <Tabs className={css.tabsRoot} orientation="vertical" selectedKey={activeId} onSelectionChange={key => { onSelect(String(key)); }}>
            <nav className={css.nav} aria-label={tx('settings:navigation.rail')} data-settings-navigation="">
              <div className={css.navTitle}>{tx('settings:settings.settings')}</div>
              <TabList className={clsx(css.navList, workflow.pageTabs)} aria-label={tx('settings:settings-root.settings-pages')}>
                {pages.map(page => (
                  <Tab key={page.id} id={page.id} className={clsx(css.navCell, workflow.pageTab)}>
                    <span className={css.navIcon} aria-hidden="true">{page.icon}</span>
                    <span className={clsx(css.navLabel, workflow.pageTabLabel)}>{page.label}</span>
                  </Tab>
                ))}
              </TabList>
            </nav>
            <div className={css.options}>
              <TabPanel key={activeId} id={activeId} className={workflow.pageTabPanel}>{children}</TabPanel>
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
