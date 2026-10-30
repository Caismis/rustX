import { createContext, useContext, useEffect, useRef, type JSX } from 'react';
import { LexicalComposer } from '@lexical/react/LexicalComposer';
import { useLexicalComposerContext } from '@lexical/react/LexicalComposerContext';
import { ContentEditable } from '@lexical/react/LexicalContentEditable';
import { LexicalErrorBoundary } from '@lexical/react/LexicalErrorBoundary';
import { PlainTextPlugin } from '@lexical/react/LexicalPlainTextPlugin';
import {
  $createLineBreakNode, $createParagraphNode, $createTextNode, $getNodeByKey, $getRoot, $getSelection, $insertNodes, $isElementNode, $isLineBreakNode,
  $isRangeSelection, COMMAND_PRIORITY_LOW, DecoratorNode, KEY_DOWN_COMMAND, type LexicalNode, type NodeKey, type SerializedLexicalNode, type Spread,
} from 'lexical';
import type { WorkflowReferenceChoice } from '../../../../../../../protocol/app-server/v62';
import type { FieldAddress } from '../../../document/fields';
import type { Value } from '../../../document/model';
import type { Edit } from '../panels/shared';
import { ReferencePicker } from './ReferencePicker';
import css from '../Editor.module.css';

/** What a variable token needs to pick from its message's native scope. */
const TokenScope = createContext<{ address: FieldAddress; edit: Edit; placeholder: string } | undefined>(undefined);

type SerializedVariable = Spread<{ path: string[] }, SerializedLexicalNode>;

/** As Dify's variable block: an inline token for one explicit reference. A
 * token inserted by `/`, `{` or the {x} button has no path yet and opens its
 * picker at once; closing that picker without a choice removes it. */
class VariableNode extends DecoratorNode<JSX.Element> {
  __path: string[];
  static getType() { return 'workflow-variable'; }
  static clone(node: VariableNode) { return new VariableNode(node.__path, node.__key); }
  static importJSON(json: SerializedVariable) { return new VariableNode(json.path); }
  constructor(path: string[], key?: NodeKey) { super(key); this.__path = path; }
  exportJSON(): SerializedVariable { return { ...super.exportJSON(), path: this.__path }; }
  createDOM() { const element = document.createElement('span'); element.className = css.promptVariable; element.contentEditable = 'false'; return element; }
  updateDOM() { return false; }
  isInline() { return true; }
  getTextContent() { return `{{${this.__path.join('.')}}}`; }
  setPath(path: string[]) { this.getWritable().__path = path; }
  decorate() { return <VariableToken nodeKey={this.__key} path={this.__path} />; }
}
const $isVariable = (node: LexicalNode | null | undefined): node is VariableNode => node instanceof VariableNode;

function VariableToken({ nodeKey, path }: { nodeKey: NodeKey; path: string[] }) {
  const [editor] = useLexicalComposerContext(), scope = useContext(TokenScope)!;
  const choose = (choice: WorkflowReferenceChoice) => editor.update(() => { const node = $getNodeByKey(nodeKey); if ($isVariable(node)) node.setPath(choice.path); });
  const closed = () => editor.update(() => { const node = $getNodeByKey(nodeKey); if ($isVariable(node) && !node.__path.length) node.remove(); });
  return <ReferencePicker address={scope.address} path={path} edit={scope.edit} token initiallyOpen={!path.length} onChoose={choose} onClose={closed} placeholder={scope.placeholder} />;
}

/** Parts, in order: fixed text as string literals, variables as references.
 * A token still being picked has no path and is not part of the message. */
function $parts(): Value[] {
  const parts: Value[] = [];
  let text = '';
  const flush = () => { if (text) parts.push({ type: 'literal', value: text }); text = ''; };
  $getRoot().getChildren().forEach((paragraph, index) => {
    if (index) text += '\n';
    if (!$isElementNode(paragraph)) return;
    for (const child of paragraph.getChildren()) {
      if ($isVariable(child)) { if (child.__path.length) { flush(); parts.push({ type: 'reference', path: child.__path }); } }
      else if ($isLineBreakNode(child)) text += '\n';
      else text += child.getTextContent();
    }
  });
  flush();
  return parts;
}

function $fill(parts: readonly Value[]) {
  const paragraph = $createParagraphNode();
  for (const part of parts) {
    if (part.type === 'reference') paragraph.append(new VariableNode([...part.path]));
    else if (part.type === 'literal' && typeof part.value === 'string') part.value.split('\n').forEach((line, index) => {
      if (index) paragraph.append($createLineBreakNode());
      if (line) paragraph.append($createTextNode(line));
    });
  }
  $getRoot().clear().append(paragraph);
}

/** Insert a token that opens its picker, as Dify's `/` and `{` triggers do. */
export function useInsertVariable() {
  const [editor] = useLexicalComposerContext();
  return () => editor.update(() => {
    if (!$isRangeSelection($getSelection())) $getRoot().selectEnd();
    $insertNodes([new VariableNode([])]);
  });
}

function Sync({ parts, onChange }: { parts: readonly Value[]; onChange: (parts: Value[]) => void }) {
  const [editor] = useLexicalComposerContext(), insert = useInsertVariable();
  const shown = useRef(JSON.stringify(parts)), latest = useRef(onChange);
  latest.current = onChange;
  // Undo, redo and other edits replace the editor's content; its own edits
  // already match the document and are left alone.
  useEffect(() => {
    const next = JSON.stringify(parts);
    if (next === shown.current) return;
    shown.current = next;
    editor.update(() => $fill(parts), { tag: 'workflow-document' });
  }, [editor, parts]);
  useEffect(() => editor.registerUpdateListener(({ editorState, tags, dirtyElements, dirtyLeaves }) => {
    if (tags.has('workflow-document') || (!dirtyElements.size && !dirtyLeaves.size)) return;
    const next = editorState.read($parts), text = JSON.stringify(next);
    if (text === shown.current) return;
    shown.current = text;
    latest.current(next);
  }), [editor]);
  useEffect(() => editor.registerCommand(KEY_DOWN_COMMAND, event => {
    if ((event.key !== '/' && event.key !== '{') || event.isComposing || event.ctrlKey || event.metaKey || event.altKey) return false;
    event.preventDefault();
    insert();
    return true;
  }, COMMAND_PRIORITY_LOW), [editor]);
  return null;
}

/** One message's content as Dify's prompt editor: plain text with inline
 * variable tokens. `children` (the card around it) can reach the editor. */
export function PromptEditor({ parts, address, edit, label, placeholder, variablePlaceholder, expanded, compact, invalid, onChange, children }: {
  parts: readonly Value[]; address: FieldAddress; edit: Edit; label: string; placeholder: string; variablePlaceholder: string;
  expanded?: boolean; compact?: boolean; invalid?: boolean; onChange: (parts: Value[]) => void; children: (editable: JSX.Element) => JSX.Element;
}) {
  const initial = useRef(parts);
  return <LexicalComposer initialConfig={{
    namespace: 'workflow-prompt', nodes: [VariableNode], onError: error => { throw error; },
    editorState: () => $fill(initial.current),
  }}>
    <TokenScope.Provider value={{ address, edit, placeholder: variablePlaceholder }}>
      {children(<div className={css.promptEditor}>
        <PlainTextPlugin ErrorBoundary={LexicalErrorBoundary}
          contentEditable={<ContentEditable className={`${css.promptInput} ${expanded ? css.promptInputExpanded : compact ? css.promptInputCompact : ''}`} aria-label={label} aria-invalid={invalid || undefined} aria-placeholder={placeholder}
            placeholder={<span className={css.promptPlaceholder}>{placeholder}</span>} />} />
      </div>)}
      <Sync parts={parts} onChange={onChange} />
    </TokenScope.Provider>
  </LexicalComposer>;
}
