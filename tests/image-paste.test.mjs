import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';

test('image paste keeps retry identity, original draft, and course-scoped safe previews', async () => {
 const dom = new JSDOM('<div id="root"></div>', { url: 'http://127.0.0.1/', pretendToBeVisual: true });
 Object.assign(globalThis, { window: dom.window, Window: dom.window.Window, Node: dom.window.Node, HTMLElement: dom.window.HTMLElement, getComputedStyle: dom.window.getComputedStyle, document: dom.window.document, MutationObserver: dom.window.MutationObserver, requestAnimationFrame: dom.window.requestAnimationFrame.bind(dom.window), cancelAnimationFrame: dom.window.cancelAnimationFrame.bind(dom.window), IS_REACT_ACT_ENVIRONMENT: true });
 Object.defineProperty(globalThis, 'navigator', { configurable: true, value: dom.window.navigator });
 dom.window.Range.prototype.getClientRects = () => [];
 dom.window.Range.prototype.getBoundingClientRect = () => ({ left: 0, right: 0, top: 0, bottom: 0 });
 const filename = new URL(`./.image-paste-${process.pid}.mjs`, import.meta.url);
 await build({ stdin: { contents: `import React from 'react'; import { useImagePaste } from './src/ImagePaste'; import { useChatDraft } from './src/useChatDraft'; import { Editor } from './src/Editor';
 export { clipboardImage, insertPastedImage } from './src/ImagePaste'; export { Markdown } from './src/Markdown';
 export function EditorHarness() { const [body,setBody]=React.useState('ab'); return <Editor ref={value=>globalThis.imageEditor=value} courseId="editor_course" initial={body} onChange={setBody} onSelection={()=>{}} onSave={()=>{}}/>; }
 export function Harness({ id }) { const paste = useImagePaste(); const draft = useChatDraft(id); globalThis.pasteController=paste; globalThis.draftController=draft; return <>{paste.status}<p>{draft.question}</p></>; }`, resolveDir: process.cwd(), loader: 'tsx' }, outfile: fileURLToPath(filename), bundle: true, packages: 'external', platform: 'node', format: 'esm', jsx: 'automatic', plugins: [{ name: 'image-fixture', setup(b) {
  b.onResolve({ filter: /^\.\/api$/ }, () => ({ path: 'image-ipc', namespace: 'fixture' }));
  b.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: 'export const nativeAvailable=true; export const recordMetric=()=>{}; export const call=(...args)=>globalThis.imageCall(...args);', loader: 'js' }));
 }}] });
 const React = await import('react'); const { createRoot } = await import('react-dom/client');
 const { Harness, EditorHarness, Markdown, clipboardImage, insertPastedImage } = await import(filename.href);
 const root = createRoot(document.getElementById('root'));
 const file = { name: 'pasted.png', type: 'image/png', size: 3, arrayBuffer: async () => new Uint8Array([1, 2, 3]).buffer };
 const imports = [], drafts = [], reads = [];
 let failImport = true, finishRead, finishImport;
 globalThis.imageCall = async (command, payload) => {
  if (command === 'draft_get') return { data: { question: payload.key === 'chat:a' ? 'before after' : 'other conversation' } };
  if (command === 'draft_save') { drafts.push(payload); return {}; }
  if (command === 'asset_import') { imports.push(payload); if (failImport) throw Error('确认失败，请重试'); if (payload.courseId === 'editor_course') return new Promise(resolve => { finishImport = resolve; }); return { id: 'asset_one', fileName: file.name, mime: file.type }; }
  if (command === 'asset_read') { reads.push(payload); if (payload.courseId === 'first') return new Promise(resolve => { finishRead = resolve; }); return { dataUrl: payload.courseId === 'unsafe' ? 'https://remote.invalid/a.png' : 'data:image/png;base64,AQID' }; }
  throw Error(command);
 };
 const render = async element => { await React.act(async () => root.render(element)); };
 try {
  assert.equal(clipboardImage({ items: [{ kind: 'string', type: 'text/plain' }, { kind: 'file', type: 'image/png', getAsFile: () => file }] }), file);
  assert.equal(clipboardImage({ items: [{ kind: 'string', type: 'text/plain' }] }), undefined);
  assert.equal(insertPastedImage('before after', 'before after', 7, 7, 'IMAGE'), 'before IMAGEafter');
  assert.equal(insertPastedImage('edited', 'before after', 7, 7, 'IMAGE'), 'edited\nIMAGE');
  await render(React.createElement(Harness, { id: 'a' }));
  const original = draftController.question;
  await React.act(async () => pasteController.paste(file, 'course_a', citation => draftController.setQuestion(value => insertPastedImage(value, original, 7, 7, citation), 'a')));
  assert.match(document.body.textContent, /确认失败/);
  await render(React.createElement(Harness, { id: 'b' }));
  failImport = false;
  await React.act(async () => [...document.querySelectorAll('button')].find(button => button.textContent === '重试图片导入').click());
  assert.equal(imports.length, 2); assert.equal(imports[0].requestId, imports[1].requestId);
  assert.equal(imports[1].courseId, 'course_a'); assert.deepEqual(imports[1].bytes, [1, 2, 3]);
  assert.equal(draftController.question, 'other conversation');
  assert.equal(drafts.find(value => value.key === 'chat:a').data.question, 'before ![pasted.png](asset:asset_one)after');
  await render(React.createElement(Harness, { id: 'a' }));
  assert.match(draftController.question, /asset:asset_one/);
  await React.act(async () => pasteController.paste({ ...file, size: 6 * 1024 * 1024 }, 'course_a', () => assert.fail('oversize inserted')));
  assert.match(document.body.textContent, /5MiB/); assert.equal(imports.length, 2);
  await React.act(async () => pasteController.paste({ ...file, type: 'image/svg+xml' }, 'course_a', () => assert.fail('svg inserted')));
  assert.match(document.body.textContent, /PNG/); assert.equal(imports.length, 2);
  const text = '![pic](asset:asset_one)\n\n![remote](https://remote.invalid/pic)';
  await render(React.createElement(Markdown, { text, courseId: 'first' }));
  await render(React.createElement(Markdown, { text, courseId: 'second' }));
  await React.act(async () => finishRead({ dataUrl: 'data:image/png;base64,AAAA' }));
  assert.equal(document.querySelectorAll('img').length, 1);
  assert.equal(document.querySelector('img').src, 'data:image/png;base64,AQID');
  assert.deepEqual(reads.map(value => value.courseId), ['first', 'second']);
  await render(React.createElement(Markdown, { text, courseId: 'unsafe' }));
  assert.equal(document.querySelector('img'), null); assert.match(document.body.textContent, /无法预览/);
  await render(React.createElement(Markdown, { text: '`![code](asset:not_read)`', courseId: 'second' }));
  assert.equal(reads.some(value => value.assetId === 'not_read'), false);
  await render(React.createElement(EditorHarness));
  await React.act(async () => imageEditor.insertText('x'));
  const paste = new dom.window.Event('paste', { bubbles: true, cancelable: true });
  Object.defineProperty(paste, 'clipboardData', { value: { items: [{ kind: 'file', type: file.type, getAsFile: () => file }] } });
  await React.act(async () => document.querySelector('.cm-content').dispatchEvent(paste));
  assert.equal(paste.defaultPrevented, true);
  await React.act(async () => imageEditor.insertText('y'));
  await React.act(async () => finishImport({ id: 'asset_one', fileName: file.name, mime: file.type }));
  assert.equal(imageEditor.getValue(), 'xy![pasted.png](asset:asset_one)ab');
  assert.equal(document.querySelector('.image-attachments img').src, 'data:image/png;base64,AQID');
 } finally { await React.act(async () => root.unmount()); dom.window.close(); await unlink(filename); }
});
