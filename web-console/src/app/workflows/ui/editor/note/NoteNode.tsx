import { Menu } from '@base-ui/react/menu';
import { Popover } from '@base-ui/react/popover';
import { autoUpdate, flip, offset, shift, useFloating } from '@floating-ui/react-dom';
import { $isLinkNode, LinkNode, TOGGLE_LINK_COMMAND } from '@lexical/link';
import { $isListItemNode, INSERT_UNORDERED_LIST_COMMAND, ListItemNode, ListNode } from '@lexical/list';
import { LexicalComposer } from '@lexical/react/LexicalComposer';
import { useLexicalComposerContext } from '@lexical/react/LexicalComposerContext';
import { ContentEditable } from '@lexical/react/LexicalContentEditable';
import { LexicalErrorBoundary } from '@lexical/react/LexicalErrorBoundary';
import { HistoryPlugin } from '@lexical/react/LexicalHistoryPlugin';
import { LinkPlugin } from '@lexical/react/LexicalLinkPlugin';
import { ListPlugin } from '@lexical/react/LexicalListPlugin';
import { RichTextPlugin } from '@lexical/react/LexicalRichTextPlugin';
import { $getSelectionStyleValueForProperty, $isAtNodeEnd, $patchStyleText, $setBlocksType } from '@lexical/selection';
import { $createParagraphNode, $getNodeByKey, $getRoot, $getSelection, $isParagraphNode, $isRangeSelection, FORMAT_TEXT_COMMAND, type EditorThemeClasses, type RangeSelection } from 'lexical';
import clsx from 'clsx';
import { createContext, memo, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { WorkflowNote, WorkflowNoteTheme } from '../../../../../../../protocol/app-server/v62';
import { useTranslation } from '../../../../../locale/react';
import { ShortcutKeys } from '../../../../../presentation/primitives/ShortcutKeys';
import { IconCheckOutline16, IconEllipsisOutline16 } from '../../../../../presentation/primitives/icons';
import { IconBold, IconBulletList, IconExternal, IconFontSize, IconItalic, IconLink, IconPencil, IconStrikethrough, IconUnlink } from '../../icons';
import { COPY_KEYS, DELETE_KEYS, DUPLICATE_KEYS } from '../shortcuts';
import css from './Note.module.css';

/** What a note does to the document; the canvas owns selection and history. */
export const NoteActions = createContext<{
  editable: boolean;
  change(id: string, change: Partial<Pick<WorkflowNote, 'text' | 'theme'>>): void;
  copy(id: string): void;
  duplicate(id: string): void;
  remove(id: string): void;
}>({ editable: false, change() {}, copy() {}, duplicate() {}, remove() {} });

const THEMES: readonly WorkflowNoteTheme[] = ['blue', 'cyan', 'green', 'yellow', 'pink', 'violet'];
const FONT_SIZES = [['12px', 'small'], ['14px', 'medium'], ['16px', 'large']] as const;
const DEFAULT_FONT_SIZE = '12px';
// Dify's note link check.
const URL_PATTERN = /((([A-Za-z]{3,9}:(?:\/\/)?)(?:[-;:&=+$,\w]+@)?[A-Za-z0-9.-]+|(?:www.|[-;:&=+$,\w]+@)[A-Za-z0-9.-]+)((?:\/[+~%/.\w-]*)?\??[-+=&;%@.\w]*#?\w*))/;
const THEME: EditorThemeClasses = {
  paragraph: css.paragraph,
  list: { ul: css.list, listitem: css.listItem },
  link: clsx(css.link, 'nodrag nopan nowheel'),
  text: { bold: css.bold, italic: css.italic, strikethrough: css.strikethrough },
};

/** As Dify's NoteNode: a rich-text note written in place on the canvas. Its
 * text is the Lexical editor state as JSON; selecting it shows the toolbar. */
export const NoteNode = memo(function NoteNode({ id, note, selected, resizer }: { id: string; note: WorkflowNote; selected: boolean; resizer: ReactNode }) {
  const tx = useTranslation(), { editable } = useContext(NoteActions), editing = selected && editable;
  return <div className={clsx(css.note, css[note.theme], selected && css.selected)}>
    <LexicalComposer initialConfig={{ namespace: 'workflow-note', nodes: [LinkNode, ListNode, ListItemNode], editorState: note.text || null, theme: THEME, editable, onError: error => { throw error; } }}>
      {resizer}
      <div className={css.titleBar} />
      <div className={clsx(css.body, selected && 'nodrag nopan nowheel')}>
        <div className={css.editor}>
          <RichTextPlugin ErrorBoundary={LexicalErrorBoundary}
            contentEditable={<ContentEditable className={clsx(css.editable, selected && css.editing)} aria-label={tx('workflows:note.title')} spellCheck={false} />}
            placeholder={<div className={css.placeholder}>{tx('workflows:note.placeholder')}</div>} />
        </div>
      </div>
      <LinkPlugin />
      <ListPlugin />
      <HistoryPlugin />
      <Sync id={id} text={note.text} />
      {editing && <Tools id={id} theme={note.theme} />}
    </LexicalComposer>
  </div>;
});

/** Writes the editor back to the note, and takes the note's text when it
 * changes from outside, such as an undo. Echoes of its own writes, which may
 * arrive after later keystrokes, are not outside changes. An empty note
 * writes no state. */
function Sync({ id, text }: { id: string; text: string }) {
  const [editor] = useLexicalComposerContext(), actions = useContext(NoteActions), written = useRef([text]);
  useEffect(() => editor.registerUpdateListener(({ editorState, dirtyElements, dirtyLeaves }) => {
    if (!dirtyElements.size && !dirtyLeaves.size) return;
    const next = editorState.read(() => {
      const root = $getRoot(), only = root.getFirstChild();
      return root.getChildrenSize() <= 1 && (!only || $isParagraphNode(only) && only.isEmpty()) ? '' : JSON.stringify(editorState.toJSON());
    });
    if (next === written.current.at(-1)) return;
    written.current.push(next);
    actions.change(id, { text: next });
  }), [editor, id, actions]);
  useEffect(() => {
    const echo = written.current.indexOf(text);
    if (echo >= 0) { written.current = written.current.slice(echo); return; }
    written.current = [text];
    if (text) editor.setEditorState(editor.parseEditorState(text));
    else editor.update(() => { $getRoot().clear(); });
  }, [editor, text]);
  return null;
}

interface Format { bold: boolean; italic: boolean; strikethrough: boolean; bullet: boolean; fontSize: string; link?: { key: string; url: string } }

function selectedNode(selection: RangeSelection) {
  const anchor = selection.anchor.getNode(), focus = selection.focus.getNode();
  if (anchor === focus) return anchor;
  return selection.isBackward() ? $isAtNodeEnd(selection.focus) ? anchor : focus : $isAtNodeEnd(selection.anchor) ? anchor : focus;
}

/** As Dify's format detector: the marks at the caret. */
function useFormat(): Format {
  const [editor] = useLexicalComposerContext();
  const [format, setFormat] = useState<Format>({ bold: false, italic: false, strikethrough: false, bullet: false, fontSize: DEFAULT_FONT_SIZE });
  useEffect(() => editor.registerUpdateListener(({ editorState }) => editorState.read(() => {
    const selection = $getSelection();
    if (!$isRangeSelection(selection)) return;
    const node = selectedNode(selection), parent = node.getParent();
    const link = $isLinkNode(node) ? node : $isLinkNode(parent) ? parent : undefined;
    let item = node; while (!$isListItemNode(item) && item.getParent()) item = item.getParent()!;
    setFormat({
      bold: selection.hasFormat('bold'), italic: selection.hasFormat('italic'), strikethrough: selection.hasFormat('strikethrough'), bullet: $isListItemNode(item),
      fontSize: $getSelectionStyleValueForProperty(selection, 'font-size', DEFAULT_FONT_SIZE), link: link && { key: link.getKey(), url: link.getURL() },
    });
  })), [editor]);
  return format;
}

/** As Dify's NoteEditorToolbar: color, font size, marks, link, list and more. */
function Tools({ id, theme }: { id: string; theme: WorkflowNoteTheme }) {
  const tx = useTranslation(), [editor] = useLexicalComposerContext(), actions = useContext(NoteActions), format = useFormat();
  const [colors, setColors] = useState(false), [sizes, setSizes] = useState(false);
  const command = (label: string, pressed: boolean, icon: ReactNode, run: () => void) =>
    <button type="button" className={clsx(css.command, pressed && css.pressed)} aria-label={label} title={label} aria-pressed={pressed}
      onMouseDown={event => event.preventDefault()} onClick={run}>{icon}</button>;
  const link = () => {
    if (format.link) { unlink(format.link.key); return; }
    editor.dispatchCommand(TOGGLE_LINK_COMMAND, '');
  };
  const unlink = (key: string) => editor.update(() => {
    const node = $getNodeByKey(key);
    if (!$isLinkNode(node)) return;
    for (const child of node.getChildren()) node.insertBefore(child);
    node.remove();
  });
  const bullet = () => {
    if (!format.bullet) { editor.dispatchCommand(INSERT_UNORDERED_LIST_COMMAND, undefined); return; }
    editor.update(() => { const selection = $getSelection(); if ($isRangeSelection(selection)) $setBlocksType(selection, () => $createParagraphNode()); });
  };
  const size = (value: string) => editor.update(() => { const selection = $getSelection(); if ($isRangeSelection(selection)) $patchStyleText(selection, { 'font-size': value }); });
  const sizeLabel = tx(`workflows:note.${FONT_SIZES.find(([value]) => value === format.fontSize)?.[1] ?? 'small'}`);
  return <>
    <div className={clsx(css.toolbar, 'nodrag nopan nowheel')} role="toolbar" aria-label={tx('workflows:note.title')} onMouseDown={event => event.stopPropagation()} onClick={event => event.stopPropagation()}>
      <Popover.Root open={colors} onOpenChange={setColors}>
        <Popover.Trigger className={css.command} aria-label={tx('workflows:note.color')} title={tx('workflows:note.color')}><span className={clsx(css.swatch, css[theme])} /></Popover.Trigger>
        <Popover.Portal><Popover.Positioner side="top" sideOffset={4} className={css.positioner}><Popover.Popup className={css.colors}>
          {THEMES.map(value => <button key={value} type="button" className={clsx(css.color, css[value], value === theme && css.current)} aria-label={tx(`workflows:note.${value}`)} title={tx(`workflows:note.${value}`)}
            onClick={() => { actions.change(id, { theme: value }); setColors(false); }}><span className={css.swatch} /></button>)}
        </Popover.Popup></Popover.Positioner></Popover.Portal>
      </Popover.Root>
      <span className={css.divider} />
      <Popover.Root open={sizes} onOpenChange={setSizes}>
        <Popover.Trigger className={css.fontSize} aria-label={tx('workflows:note.font-size')}><IconFontSize size={16} />{sizeLabel}</Popover.Trigger>
        <Popover.Portal><Popover.Positioner side="bottom" align="start" sideOffset={2} className={css.positioner}><Popover.Popup className={css.sizes} initialFocus={false} finalFocus={false}>
          {FONT_SIZES.map(([value, name]) => <button key={value} type="button" className={css.size} onMouseDown={event => event.preventDefault()} onClick={() => { size(value); setSizes(false); }}>
            <span style={{ fontSize: value }}>{tx(`workflows:note.${name}`)}</span>{format.fontSize === value && <IconCheckOutline16 />}
          </button>)}
        </Popover.Popup></Popover.Positioner></Popover.Portal>
      </Popover.Root>
      <span className={css.divider} />
      {command(tx('workflows:note.bold'), format.bold, <IconBold size={16} />, () => editor.dispatchCommand(FORMAT_TEXT_COMMAND, 'bold'))}
      {command(tx('workflows:note.italic'), format.italic, <IconItalic size={16} />, () => editor.dispatchCommand(FORMAT_TEXT_COMMAND, 'italic'))}
      {command(tx('workflows:note.strikethrough'), format.strikethrough, <IconStrikethrough size={16} />, () => editor.dispatchCommand(FORMAT_TEXT_COMMAND, 'strikethrough'))}
      {command(tx('workflows:note.link'), !!format.link, <IconLink size={16} />, link)}
      {command(tx('workflows:note.bullet-list'), format.bullet, <IconBulletList size={16} />, bullet)}
      <span className={css.divider} />
      <Menu.Root modal={false}>
        <Menu.Trigger className={css.command} aria-label={tx('workflows:note.more')} title={tx('workflows:note.more')}><IconEllipsisOutline16 /></Menu.Trigger>
        <Menu.Portal><Menu.Positioner side="bottom" align="end" sideOffset={4} className={css.positioner}><Menu.Popup className={css.menu}>
          <Menu.Item className={css.menuItem} onClick={() => actions.copy(id)}>{tx('workflows:canvas.copy')}<ShortcutKeys keys={COPY_KEYS} /></Menu.Item>
          <Menu.Item className={css.menuItem} onClick={() => actions.duplicate(id)}>{tx('workflows:canvas.duplicate')}<ShortcutKeys keys={DUPLICATE_KEYS} /></Menu.Item>
          <Menu.Separator className={css.separator} />
          <Menu.Item className={clsx(css.menuItem, css.destructive)} onClick={() => actions.remove(id)}>{tx('workflows:note.delete')}<ShortcutKeys keys={DELETE_KEYS} /></Menu.Item>
        </Menu.Popup></Menu.Positioner></Menu.Portal>
      </Menu.Root>
    </div>
    {format.link && <LinkBar key={format.link.key} link={format.link} unlink={unlink} />}
  </>;
}

/** As Dify's link editor: an empty link asks for its address; a set one
 * opens, edits or unlinks. Dismissing an empty link removes it. */
function LinkBar({ link, unlink }: { link: { key: string; url: string }; unlink: (key: string) => void }) {
  const tx = useTranslation(), [editor] = useLexicalComposerContext();
  const [editing, setEditing] = useState(!link.url), [url, setUrl] = useState(link.url), [dismissed, setDismissed] = useState(false), [invalid, setInvalid] = useState(false);
  const bar = useRef<HTMLDivElement>(null);
  const { refs, floatingStyles } = useFloating({ placement: 'top', strategy: 'fixed', whileElementsMounted: autoUpdate, middleware: [offset(4), shift({ padding: 8 }), flip()] });
  useEffect(() => { refs.setReference(editor.getElementByKey(link.key)); }, [editor, link.key, refs]);
  const dismiss = () => { setDismissed(true); if (!link.url) unlink(link.key); };
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!bar.current?.contains(target) && !editor.getElementByKey(link.key)?.contains(target)) dismiss();
    };
    window.document.addEventListener('pointerdown', outside, true);
    return () => window.document.removeEventListener('pointerdown', outside, true);
  });
  const save = () => {
    if (!URL_PATTERN.test(url)) { setInvalid(true); return; }
    editor.update(() => { const node = $getNodeByKey(link.key); if ($isLinkNode(node)) node.setURL(url); });
    setEditing(false);
  };
  if (dismissed) return null;
  return createPortal(<div ref={element => { bar.current = element; refs.setFloating(element); }} style={floatingStyles} className={clsx(css.linkBar, !editing && css.linkOperator)}
    onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); dismiss(); editor.focus(); } }}>
    {editing ? <>
      <input className={css.linkInput} value={url} autoFocus aria-label={tx('workflows:note.link')} aria-invalid={invalid || undefined} placeholder={tx('workflows:note.enter-url')} title={invalid ? tx('workflows:note.invalid-url') : undefined}
        onChange={event => { setUrl(event.target.value); setInvalid(false); }} onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); if (url) save(); } }} />
      <button type="button" className={css.primary} disabled={!url} onClick={save}>{tx('workflows:note.ok')}</button>
    </> : <>
      <a className={css.open} href={link.url} target="_blank" rel="noopener noreferrer"><IconExternal size={12} />{tx('workflows:note.open-link')}<span title={link.url}>{link.url}</span></a>
      <span className={css.divider} />
      <button type="button" className={css.ghost} onClick={() => setEditing(true)}><IconPencil size={12} />{tx('workflows:note.edit-link')}</button>
      <button type="button" className={css.ghost} onClick={() => { unlink(link.key); setDismissed(true); }}><IconUnlink size={12} />{tx('workflows:note.unlink')}</button>
    </>}
  </div>, window.document.body);
}
