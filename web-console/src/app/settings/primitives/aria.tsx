import { useId, useRef, useState, type Key, type ReactNode } from 'react';
import { flushSync } from 'react-dom';
import {
  Button as AriaButton, Dialog, Disclosure, DisclosurePanel, GridList, GridListItem, Heading, Input, Label,
  ListBox, ListBoxItem, Menu, MenuItem, MenuTrigger, Modal, ModalOverlay, Popover, SearchField, Select, SelectValue,
  Switch as AriaSwitch, Tab, TabList, TabPanel, Tabs,
} from 'react-aria-components';
import { Button } from '../../../presentation/primitives/Button';
import { IconTrashOutline16 } from '../../../presentation/primitives/icons';
import buttonCss from '../../../presentation/primitives/Button.module.css';
import css from '../../../presentation/settings/SettingsWorkflow.module.css';

/** The bounded React Aria adoption of the Settings workflows.
 *
 * React Aria Components own interaction and accessibility semantics here and
 * nothing else: keyboard navigation and roving focus, dialog focus containment
 * and restoration, menu/list/select interaction, accessible tabs, disclosure
 * behavior and search-field semantics. Every visual decision stays in the
 * existing Harness-derived rustX token family, so no competing design system
 * is imported and no Spectrum stylesheet is loaded.
 *
 * These wrappers exist because the same interaction appears many times across
 * the six pages — a resource list, an enum, a destructive confirmation, an
 * advanced section. A primitive used exactly once is used directly at its call
 * site instead, so this module never becomes a speculative design system. */

/** A closed enum. Native remains the authority on which values are legal; this
 * control only presents the ones the call site offers. */
export function Choice<T extends string>({ label, value, onChange, options, disabled = false, description }: {
  label: string; value: T; onChange: (value: T) => void;
  options: readonly (readonly [T, string])[]; disabled?: boolean; description?: ReactNode;
}) {
  return <Select className={css.select} selectedKey={value} isDisabled={disabled} onSelectionChange={key => onChange(key as T)}>
    <Label>{label}</Label>
    <AriaButton className={css.selectTrigger}><SelectValue /><span aria-hidden="true">▾</span></AriaButton>
    {description}
    <Popover className={css.popover}>
      <ListBox>{options.map(([key, text]) => <ListBoxItem key={key} id={key} className={css.option}>{text}</ListBoxItem>)}</ListBox>
    </Popover>
  </Select>;
}
/** A resource search box. Filtering is presentation; the identities it filters
 * are always the native ones the page was given. */
export function Search({ label, value, onChange, placeholder }: {
  label: string; value: string; onChange: (value: string) => void; placeholder?: string;
}) {
  return <SearchField className={css.search} value={value} onChange={onChange} aria-label={label}>
    <Input placeholder={placeholder} />
  </SearchField>;
}

/** One row of a resource list. Identity and the native facts about it are
 * separate fields on purpose: a list never collapses kind, ownership, validity,
 * preparation and availability into one badge. */
export interface ResourceRow {
  readonly id: string;
  readonly name: string;
  /** The native facts, each rendered as its own element. */
  readonly facts: ReactNode;
  readonly detail?: ReactNode;
  /** Contextual actions. Each submits through the same native mutation
   * identity and transaction owner as its page's primary editor. */
  readonly actions?: readonly RowAction[];
}
export interface RowAction { readonly id: string; readonly label: string; readonly danger?: boolean; readonly run: () => void }

/** A resource list with pointer and keyboard parity.
 *
 * React Aria owns arrow-key navigation, typeahead and the Enter/Space action,
 * so opening a detail from the keyboard is the same navigation decision as
 * clicking it, and both reach the one navigation owner. */
export function ResourceList({ label, rows, selected, onOpen, empty = 'No matching definitions in this native projection.' }: {
  label: string; rows: readonly ResourceRow[]; selected?: string;
  onOpen: (id: string) => void; empty?: ReactNode;
}) {
  if (!rows.length) return <p className={css.empty}>{empty}</p>;
  return <GridList className={css.list} aria-label={label} selectionMode="single"
    selectedKeys={selected ? [selected] : []} onAction={key => onOpen(String(key))}>
    {rows.map(row => <GridListItem key={row.id} id={row.id} className={css.row} textValue={row.name} aria-label={row.name}>
      {/* The identity and its contextual actions share the first line; the
          native facts follow as secondary badges, then any detail. */}
      <div className={css.rowTop}>
        <span className={css.rowName}>{row.name}</span>
        {!!row.actions?.length && <RowActions label={`Actions for ${row.name}`} actions={row.actions} />}
      </div>
      <span className={css.rowFacts}>{row.facts}</span>
      {row.detail}
    </GridListItem>)}
  </GridList>;
}

/** The contextual action menu of one resource. Its trigger is a React Aria
 * button so that pressing it is handled by the menu and never also activates
 * the surrounding row. */
export function RowActions({ label, actions }: { label: string; actions: readonly RowAction[] }) {
  return <MenuTrigger>
    <AriaButton aria-label={label} className={`${buttonCss.button} ${buttonCss.ghost} ${buttonCss.sm}`}>⋯</AriaButton>
    <Popover className={css.popover}>
      <Menu aria-label={label} onAction={key => actions.find(action => action.id === key)?.run()}>
        {actions.map(action => <MenuItem key={action.id} id={action.id} className={css.menuItem} data-danger={action.danger || undefined}>{action.label}</MenuItem>)}
      </Menu>
    </Popover>
  </MenuTrigger>;
}

/** An advanced or secondary section. Progressive disclosure is how Settings
 * keeps advanced detail reachable without creating another primary page. */
export function Advanced({ title, children, expanded }: { title: string; children: ReactNode; expanded?: boolean }) {
  return <Disclosure className={css.disclosure} defaultExpanded={expanded}>
    <Heading level={4} style={{ margin: 0 }}>
      <AriaButton slot="trigger" className={css.disclosureButton}><span className={css.disclosureMarker} aria-hidden="true">▸</span>{title}</AriaButton>
    </Heading>
    <DisclosurePanel className={css.disclosurePanel}>{children}</DisclosurePanel>
  </Disclosure>;
}

/** A boolean enablement control. */
export function Toggle({ label, checked, onChange, disabled = false }: {
  label: string; checked: boolean; onChange: (value: boolean) => void; disabled?: boolean;
}) {
  return <div className={css.switchRow}>
    <span>{label}</span>
    <AriaSwitch className={css.switch} isSelected={checked} isDisabled={disabled} onChange={onChange} aria-label={label}>
      <span className={css.switchTrack} />
    </AriaSwitch>
  </div>;
}

/** A confirmation of one native removal.
 *
 * `description` must describe the actual native consequence of the exact
 * mutation the confirmation submits and nothing else — never a guess about
 * running Attempts, live runtimes, existing Sessions or adopted bindings.
 *
 * `tone` says which consequence that is. A `destructive` confirmation removes
 * something that nothing replaces: it is an `alertdialog` whose trigger and
 * confirm action carry the destructive color. A `restore` confirmation removes
 * an override so that the inherited value applies again: it is an ordinary
 * `dialog` with an ordinary primary action, and it never looks like deletion.
 * The destructive trigger carries the existing trash glyph; its label, like
 * every label, keeps full contrast.
 *
 * This is a transient layer over the one Settings modal root, not a second
 * Settings tree: it mounts only while the confirmation is open, React Aria
 * contains focus inside it, and dismissing it restores focus to the trigger.
 * Escape closes this topmost layer only. */
export function ConfirmAction({ label, title, description, confirm, onConfirm, tone, disabled = false }: {
  label: string; title: string; description: ReactNode; confirm: string;
  onConfirm: () => void; tone: 'destructive' | 'restore'; disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const titleId = useId();
  const destructive = tone === 'destructive';
  const seat = useRef<HTMLSpanElement>(null);
  /** Closing hands focus straight back to the trigger. The layer is unmounted
   * synchronously first, so its focus scope is gone and the trigger's focus is
   * never contested; focus therefore never falls to `<body>`, where React
   * Aria's restoration and the Settings scope's containment would otherwise
   * race over the next animation frame to decide where it lands. */
  const change = (next: boolean) => {
    if (next) { setOpen(true); return; }
    flushSync(() => setOpen(false));
    seat.current?.querySelector<HTMLButtonElement>(':scope > button')?.focus();
  };
  return <span ref={seat} className={css.removal} data-tone={tone}>
    <Button type="button" disabled={disabled} className={destructive ? css.dangerTrigger : undefined}
      icon={destructive ? <IconTrashOutline16 /> : undefined} onClick={() => change(true)}>{label}</Button>
    <ModalOverlay className={css.confirmOverlay} isOpen={open} onOpenChange={change} isDismissable>
      <Modal className={css.confirmModal}>
        <Dialog className={css.confirmDialog} role={destructive ? 'alertdialog' : 'dialog'} aria-labelledby={titleId}>
          {({ close }) => <>
            <Heading slot="title" id={titleId}>{title}</Heading>
            {description}
            <div className={css.confirmActions}>
              {/* The least destructive action takes initial focus. React
                  Aria's Button focuses from an effect, after this layer's
                  focus scope is registered as a child of the Settings scope;
                  a native `autoFocus` fires during commit, before that, so the
                  Settings scope would read it as focus escaping and take it
                  back to the trigger. */}
              <AriaButton autoFocus className={`${buttonCss.button} ${buttonCss.outline} ${buttonCss.md}`} onPress={close}>Cancel</AriaButton>
              <AriaButton className={`${buttonCss.button} ${buttonCss.primary} ${buttonCss.md} ${destructive ? css.destructive : ''}`}
                onPress={() => { onConfirm(); close(); }}>{confirm}</AriaButton>
            </div>
          </>}
        </Dialog>
      </Modal>
    </ModalOverlay>
  </span>;
}

/** A secondary filter strip, used by the one Extensions resource surface. */
export function FilterTabs<T extends string>({ label, value, onChange, options, children }: {
  label: string; value: T; onChange: (value: T) => void;
  options: readonly (readonly [T, string])[]; children: ReactNode;
}) {
  return <Tabs selectedKey={value} onSelectionChange={(key: Key) => onChange(key as T)}>
    <TabList className={css.filterTabs} aria-label={label}>
      {options.map(([key, text]) => <Tab key={key} id={key} className={css.filterTab}>{text}</Tab>)}
    </TabList>
    <TabPanel id={value} className={css.pageTabPanel}>{children}</TabPanel>
  </Tabs>;
}
