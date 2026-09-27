import { useTranslation } from '../../locale/react';
/* Copyright (c) 2026 DeepSeek. MIT. See PROVENANCE.md. */
import { useCallback, useEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { Tab, TabList, TabPanel, Tabs } from 'react-aria-components';
import clsx from 'clsx';
import { DialogSurface } from '../primitives/DialogSurface';
import { Menu } from '../primitives/Menu';
import { IconChevronDownOutline14, IconCloseOutline16, IconSettingsOutline16 } from '../primitives/icons';
import css from './SettingsRoot.module.css';
import workflow from './SettingsWorkflow.module.css';

/** One primary Settings page as the panel presents it. The icon is a
 * recognition aid only: the label is always the page's accessible name. */
export interface SettingsPageEntry { id: string; label: string; icon: ReactNode }

/**
 * The one Settings modal root: full-viewport mask + centered panel, with the
 * primary product pages as its navigation.
 *
 * Focus containment, focus restoration to the trigger, Escape and the
 * dismissable mask belong to the shared Base UI-backed `DialogSurface`.
 * React Aria `Tabs`/`TabList`/`Tab`/`TabPanel` own
 * roving focus and arrow-key page navigation along the vertical rail.
 *
 * The panel has two layouts and no layout state. It is a size container, and
 * `SettingsRoot.module.css` decides from its own usable inline size — never
 * from the browser window — whether the rail or the section menu is shown: a
 * narrow panel hides the rail and shows a section menu in its header instead.
 * Both are always rendered, so no JavaScript breakpoint, media-query store or
 * resize observer takes part, and whichever one CSS hides is out of the
 * accessibility tree and the tab order. The section menu is the shared rustX
 * `Menu`, portaled and placed by Floating UI; like the rail, it only asks the
 * owner of navigation to select a page. Its open/closed state is the one
 * transient fact this component holds. When the panel widens while that menu
 * is open, its trigger leaves layout and the `Menu` closes itself through
 * `onClose`, so the open state never outlives the layout that shows it and
 * this component never mirrors the container query. The dialog is the menu's
 * named focus owner: a keyboard the menu held stays in Settings, on the
 * dialog, exactly where a freshly opened Settings starts, and Tab continues
 * from there into the header and the rail.
 *
 * The two presentations share one keyboard. When the navigation control that
 * holds it — a rail tab, or the closed section menu's trigger — leaves
 * rendered layout, the keyboard moves to the other presentation's control for
 * the same selected page (see `useNavigationFocusHandoff`). The panel never
 * asks whether it is narrow or wide; it answers the browser's report that a
 * focused control stopped being rendered, and the page selection is untouched.
 *
 * There is exactly one of these in the application. A confirmation inside a
 * page opens its own transient layer over this one; it never builds a second
 * Settings tree.
 */
export function SettingsPanel({ pages, activeId, onSelect, onClose, context, children }: {
  /** `activeId` is always one of `pages`: the owner of navigation admits no
   * other, so it is rendered as it is. */
  pages: readonly SettingsPageEntry[]; activeId: string; onSelect: (id: string) => void;
  /** The owner and observation state of the open Settings instance. It sits in
   * the fixed header, so every page shares it and none of it scrolls away. */
  context?: ReactNode;
  onClose: () => void; children: ReactNode;
}) {
  const tx = useTranslation();
  const [sections, setSections] = useState(false);
  const dialogRef = useRef<HTMLElement>(null);
  const [dialogNode, setDialogNode] = useState<HTMLElement | null>(null);
  const attachDialog = useCallback((node: HTMLElement | null) => {
    dialogRef.current = node;
    setDialogNode(node);
  }, []);
  const sectionTriggerRef = useRef<HTMLButtonElement>(null);
  useNavigationFocusHandoff(dialogNode, sectionTriggerRef);
  const active = pages.find(page => page.id === activeId)!;
  return (
    <DialogSurface open onClose={onClose} title={tx('settings:settings.settings')} overlayClassName={css.overlay}
      panelClassName={css.panel} className={css.dialog} contentRef={attachDialog} initialFocus={() => { dialogRef.current?.focus({ preventScroll: true }); return false; }}>
          {/* The header is deliberately outside <Tabs>: React Aria renders a
              Tabs subtree a second time into a detached collection document
              to discover its tabs, and the section menu measures real DOM. */}
          <header data-settings-dialog="" className={css.header}>
            <div className={css.sections}>
              <Menu open={sections} onClose={() => setSections(false)} autoFocus focusOwner={dialogRef}
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
            <button type="button" className={css.close} onClick={onClose}>
              <IconCloseOutline16 size={14} />
              <span className={css.hiddenLabel}>{tx('settings:settings-root.close-settings')}</span>
            </button>
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
    </DialogSurface>
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
 * that is not rendered refuses focus, and the dialog — where a freshly opened
 * Settings starts — takes it instead, so the handoff never lands on a hidden
 * control and never bounces between the two. Selection, the section menu,
 * drafts and native state are not touched.
 */
function useNavigationFocusHandoff(dialog: HTMLElement | null, sectionTriggerRef: RefObject<HTMLButtonElement | null>): void {
  useEffect(() => {
    if (dialog === null) return;
    const onFocusOut = (event: FocusEvent) => {
      const control = event.target;
      // A focus move to somewhere, a window blur, or a removal is not this.
      if (event.relatedTarget !== null || !(control instanceof HTMLElement) || !control.isConnected || control.checkVisibility()) return;
      const counterpart = control === sectionTriggerRef.current
        ? dialog.querySelector<HTMLElement>(`${RAIL_TAB}[aria-selected="true"]`)
        : control.matches(RAIL_TAB) ? sectionTriggerRef.current : undefined;
      if (counterpart === undefined) return;
      for (const owner of [counterpart, dialog]) {
        owner?.focus({ preventScroll: true });
        if (document.activeElement === owner) return;
      }
    };
    dialog.addEventListener('focusout', onFocusOut);
    return () => { dialog.removeEventListener('focusout', onFocusOut); };
  }, [dialog, sectionTriggerRef]);
}

export function SettingsTrigger({ wide, onClick }: { wide: boolean; onClick: () => void }) {
  const tx = useTranslation();
  return <div className={clsx(css.triggerRow, !wide && css.railRow)}><button type="button" className={clsx(css.trigger, !wide && css.rail)} aria-label={tx('settings:settings.settings')} aria-haspopup="dialog" onClick={onClick}>
    <IconSettingsOutline16 />{wide && <span className={css.triggerLabel}>{tx('settings:settings.settings')}</span>}
  </button></div>;
}
