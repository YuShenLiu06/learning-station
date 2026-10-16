import { forwardRef, useEffect, useImperativeHandle, useRef } from 'react';
import { recordMetric } from './api';
import { EditorState } from '@codemirror/state';
import { EditorView, drawSelection, keymap, placeholder } from '@codemirror/view';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { markdown } from '@codemirror/lang-markdown';
import { syntaxHighlighting, defaultHighlightStyle } from '@codemirror/language';

export type EditorHandle = { getValue: () => string; focus: () => void; insertText: (text: string) => void };
type Props = { initial: string; onChange: (value: string) => void; onSelection: (text: string) => void; onSave: () => void; onReady?: () => void };
export const Editor = forwardRef<EditorHandle, Props>(function Editor(props, ref) {
  const root = useRef<HTMLDivElement>(null);
  const editor = useRef<EditorView | null>(null);
  const callbacks = useRef(props);
  callbacks.current = props;
  useImperativeHandle(ref, () => ({ getValue: () => editor.current?.state.doc.toString() ?? props.initial, focus: () => editor.current?.focus(), insertText: (text: string) => { const view = editor.current; if (!view) return; const selection = view.state.selection.main; view.dispatch({ changes: { from: selection.from, to: selection.to, insert: text }, selection: { anchor: selection.from + text.length }, scrollIntoView: true }); view.focus(); } }), []);
  useEffect(() => {
    if (!root.current) return;
    const view = new EditorView({
      parent: root.current,
      state: EditorState.create({
        doc: props.initial,
        extensions: [
          history(), drawSelection(), markdown(),
          EditorView.domEventHandlers({ compositionend: () => { const started = performance.now(); requestAnimationFrame(() => recordMetric('ime-commit', performance.now() - started)); return false; } }), syntaxHighlighting(defaultHighlightStyle),
          EditorView.lineWrapping, placeholder('在这里粘贴或写下你的小节内容…'),
          keymap.of([{ key: 'Mod-s', run: () => { callbacks.current.onSave(); return true; } }, ...defaultKeymap, ...historyKeymap]),
          EditorView.contentAttributes.of({ 'aria-label': '编辑小节正文', spellcheck: 'false', autocorrect: 'off' }),
          EditorView.updateListener.of(update => {
            if (update.docChanged) callbacks.current.onChange(update.state.doc.toString());
            if (update.selectionSet) { const selection = update.state.selection.main; if (!selection.empty) callbacks.current.onSelection(update.state.sliceDoc(selection.from, selection.to)); }
          }),
          EditorView.theme({ '&': { minHeight: '100%', fontSize: 'var(--content-font-size, 16px)', background: 'transparent' }, '.cm-content': { fontFamily: 'var(--font-reading)', lineHeight: '1.95', padding: '28px 0 120px', caretColor: '#315d4a' }, '.cm-line': { padding: '0' }, '.cm-focused': { outline: 'none' }, '&.cm-focused': { outline: 'none' }, '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': { background: '#d9e7dc' }, '.cm-scroller': { overflow: 'visible' }, '.cm-gutters': { display: 'none' } }),
        ],
      }),
    });
    editor.current = view;
    const frame = requestAnimationFrame(() => callbacks.current.onReady?.());
    return () => { cancelAnimationFrame(frame); view.destroy(); editor.current = null; };
  }, []);
  useEffect(() => { const view = editor.current; if (!view || view.state.doc.toString() === props.initial) return; const selection = Math.min(view.state.selection.main.head, props.initial.length); view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: props.initial }, selection: { anchor: selection } }); }, [props.initial]);
  return <div className="editor-root" ref={root} />;
});
