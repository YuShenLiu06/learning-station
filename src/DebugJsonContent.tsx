import { useEffect, useRef, useState } from 'react';
import { EditorState } from '@codemirror/state';
import { Decoration, EditorView, keymap, lineNumbers } from '@codemirror/view';
import { HighlightStyle, foldAll, foldGutter, foldKeymap, syntaxHighlighting, unfoldAll } from '@codemirror/language';
import { tags } from '@lezer/highlight';
import { json } from '@codemirror/lang-json';
import { openSearchPanel, search, searchKeymap } from '@codemirror/search';
import type { DebugViewProps } from './DebugJsonView';

// A thin read-only CodeMirror host. Parsing, selection, search and folding are upstream.
export default function DebugJsonContent({ value, label, text: plainText, format='json' }: DebugViewProps) {
  const host = useRef<HTMLDivElement>(null);
  const editor = useRef<EditorView | null>(null);
  const [status, setStatus] = useState('');
  const text = format==='json'?(JSON.stringify(value, null, 2) ?? 'null'):plainText??'';
  useEffect(() => {
    if (!host.current) return;
    const view = new EditorView({parent: host.current, state: EditorState.create({doc: text, extensions: [
      EditorState.readOnly.of(true), EditorState.phrases.of({'Find':'查找','next':'下一个','previous':'上一个','all':'全部匹配','match case':'区分大小写','regexp':'正则','by word':'整词','close':'关闭','No matches':'无匹配'}), EditorView.editable.of(false),
      EditorView.contentAttributes.of({'aria-label': label, tabindex: '0', spellcheck: 'false'}),
      ...(format==='json'?[json(),foldGutter(),syntaxHighlighting(HighlightStyle.define([{tag:tags.propertyName,color:'#165f8c'},{tag:tags.string,color:'#8d3a25'},{tag:tags.number,color:'#1a6c53'},{tag:[tags.bool,tags.null],color:'#713b91'}]))]:[]),
      ...(format==='diff'?[EditorView.decorations.compute(['doc'],state=>{
        const marks=[];for(let n=1;n<=state.doc.lines;n++){
          const line=state.doc.line(n),kind=/^(diff --git|index |---|\+\+\+)/.test(line.text)?'header':line.text.startsWith('@@')?'hunk':line.text.startsWith('+')?'add':line.text.startsWith('-')?'remove':'';
          if(kind)marks.push(Decoration.line({class:'debug-diff-'+kind}).range(line.from));
        }return Decoration.set(marks);
      })]:[]),lineNumbers(), search({top: true}),
      keymap.of([...foldKeymap, ...searchKeymap]), EditorView.lineWrapping,
      EditorView.theme({'&': {fontSize: '13px', backgroundColor: '#fbf8f1'}, '.cm-scroller': {fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', lineHeight: '1.7', maxHeight: '60vh', overflow: 'auto'}, '.cm-content': {padding: '12px 0'}, '.cm-gutters': {backgroundColor: '#f1ebdf', color: '#726757', borderRight: '1px solid #ded4c5'}, '&.cm-focused': {outline: '2px solid #8d633f'}, '.cm-search': {fontFamily: 'inherit'},'.debug-diff-add':{backgroundColor:'#edf5e8',color:'#315f39'},'.debug-diff-remove':{backgroundColor:'#fff0eb',color:'#963e32'},'.debug-diff-header':{color:'#7d613e',fontWeight:'600'},'.debug-diff-hunk':{backgroundColor:'#eaf0f4',color:'#345d79'}}),
    ]})});
    editor.current = view;
    return () => { editor.current = null; view.destroy(); };
  }, [label,format]);
  useEffect(() => {const view = editor.current; if (view && view.state.doc.toString() !== text) {view.dispatch({changes: {from: 0, to: view.state.doc.length, insert: text}}); setStatus('');}}, [text]);
  async function copy() {try {await navigator.clipboard.writeText(text);setStatus(format==='json'?'已复制完整 JSON（已脱敏）':'已复制完整正文（已脱敏）');} catch {setStatus('复制失败，请在正文中全选复制。');}}
  return <div className="debug-json">
    <div className="debug-json-toolbar" role="group" aria-label={`${label}操作`}>
      <button className="quiet-button" onClick={()=>{if(editor.current)openSearchPanel(editor.current);}}>查找 ⌘F</button>
      {format==='json'&&<><button className="quiet-button" onClick={()=>{if(editor.current)foldAll(editor.current);}}>全部折叠</button>
      <button className="quiet-button" onClick={()=>{if(editor.current)unfoldAll(editor.current);}}>全部展开</button></>}
      <button className="quiet-button" onClick={()=>{void copy();}}>{format==='json'?'复制完整 JSON':'复制全文'}</button>
      <span role="status" className="muted small">{status || (format==='json'?'只读 · 缩进 2 空格':format==='diff'?'原生 diff · 只读':'原文 · 只读')}</span>
    </div>
    <div ref={host}/>
  </div>;
}
