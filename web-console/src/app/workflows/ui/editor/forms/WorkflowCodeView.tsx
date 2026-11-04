import { useLayoutEffect, useMemo, useRef } from 'react';
import CodeMirror, { EditorView, ExternalChange, type ReactCodeMirrorRef } from '@uiw/react-codemirror';
import { syntaxHighlighting } from '@codemirror/language';
import { python } from '@codemirror/lang-python';
import { javascript } from '@codemirror/lang-javascript';
import type { CodeLanguage } from '../../../document/code';
import { classHighlighter } from '@lezer/highlight';

const languages={python3:python(),javascript:javascript()};
const highlighting=[syntaxHighlighting(classHighlighter),EditorView.lineWrapping];
const setup={syntaxHighlighting:false,foldGutter:false,highlightActiveLine:false,highlightActiveLineGutter:false,autocompletion:false,highlightSelectionMatches:false};

export default function WorkflowCodeView({language,label,value,onChange,onBlur,field}:{language:CodeLanguage;label:string;value:string;onChange:(value:string)=>void;onBlur:()=>void;field:string}) {
  const editor=useRef<ReactCodeMirrorRef>(null);
  const extensions=useMemo(()=>[languages[language],...highlighting,EditorView.contentAttributes.of({'aria-label':label,'data-workflow-field':field,spellcheck:'false'})],[language,label,field]);
  useLayoutEffect(()=>{const view=editor.current?.view;if(view && view.state.doc.toString()!==value)view.dispatch({changes:{from:0,to:view.state.doc.length,insert:value},annotations:ExternalChange.of(true)});},[value]);
  return <CodeMirror ref={editor} value={value} onChange={onChange} onBlur={onBlur} theme="none" height="100%" extensions={extensions} basicSetup={setup} indentWithTab={false}/>;
}
