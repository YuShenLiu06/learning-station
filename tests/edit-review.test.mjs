import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';

test('inline review is contextual, protects drafts, navigates changes and disappears after decisions', async () => {
 const dom = new JSDOM('<div id="root"></div>', { url: 'http://127.0.0.1/' });
 Object.assign(globalThis, { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true,
  requestAnimationFrame: callback => setTimeout(callback, 0), cancelAnimationFrame: clearTimeout });
 let scrolls = 0; dom.window.HTMLElement.prototype.scrollIntoView = () => scrolls++;
 const filename = new URL(`./.edit-review-${process.pid}.mjs`, import.meta.url);
 await build({ stdin: { contents: `import React from 'react'; import { useEditReview, EditReviewContent, EditReviewDock, EditReviewLinks } from './src/EditReview';
 export function Harness(props) { const review=useEditReview(props); return <><article className="reading-content">{review.proposal?<EditReviewContent review={review}/>:<p>Normal document</p>}</article><EditReviewDock review={review}/><aside><EditReviewLinks review={review} title={p=>p.id==='proposal_a'?'Lesson A':'Lesson B'}/></aside></> }`, resolveDir: process.cwd(), loader: 'tsx' }, outfile: filename.pathname, bundle: true, packages: 'external', platform: 'node', format: 'esm', jsx: 'automatic', plugins: [{ name: 'isolated-review-ipc', setup(b) {
  b.onResolve({ filter: /^\.\/api$/ }, () => ({ path: 'review-ipc', namespace: 'fixture' }));
  b.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: 'export const nativeAvailable=true; export const call=(...args)=>globalThis.reviewCall(...args);', loader: 'js' }));
 }}] });
 const React = await import('react'); const { createRoot } = await import('react-dom/client');
 const { Harness } = await import(filename.href); const root = createRoot(document.getElementById('root'));
 const a = { id: 'proposal_a', courseId: 'course_a', path: 'documents/doc_a.md', original: '# Heading\n\nOld paragraph\n\nKeep [link][ref].\n\n```js\nold()\n```\n\n[ref]: https://example.com', content: '# Heading\n\nNew paragraph\n\nKeep [link][ref].\n\n```js\nnew()\n```\n\n[ref]: https://example.com', status: 'pending' };
 const b = { ...a, id: 'proposal_b', path: 'documents/doc_b.md' };
 let loaded = true, draftFails = false, dirty = true, visible = [], decisions = [], refreshed = [], notices = [], opened = [];
 globalThis.reviewCall = async (command, payload) => {
  if (command === 'edit_proposals') return visible.map(p => ({ id: p.id, stamp: '1' }));
  if (command === 'edit_proposal') return visible.find(p => p.id === payload.id);
  if (command === 'section_open') return { body: a.original };
  if (command === 'draft_get') { if (draftFails) throw Error('草稿读取失败'); return { data: { body: 'Unopened local draft' } }; }
  if (command === 'decide_edit_proposal') { decisions.push(payload); visible = visible.filter(p => p.id !== payload.id); return payload.accept ? { persisted: true } : { persisted: false, status: 'rejected' }; }
  throw Error(command);
 };
 let key = 0;
 const props = { courseId: 'course_a', documentId: 'doc_a', isDirty: () => dirty, isLoaded: () => loaded, onAccepted: async id => refreshed.push(id), onOpen: id => opened.push(id), notify: text => notices.push(text) };
 const render = async (overrides = {}, reset = false) => { if (reset) key++; await React.act(async () => root.render(React.createElement(Harness, { ...props, ...overrides, key }))); await React.act(async () => { await new Promise(r => setTimeout(r, 20)); }); };
 const button = text => [...document.querySelectorAll('button')].find(b => b.textContent === text);
 const accept = () => [...document.querySelectorAll('button')].find(b => /^接受/u.test(b.textContent));
 try {
  await render(); assert.equal(document.querySelector('.review-dock'), null); assert.equal(document.querySelector('.review-file-links'), null); assert.equal(document.querySelector('.inline-edit-review'), null);
  visible = [a, b]; await render({}, true);
  assert.equal(document.querySelector('.reading-content h1').textContent, 'Heading');
  assert.equal(document.querySelector('.reading-content .review-removed pre code').textContent.trim(), 'old()');
  assert.equal(document.querySelector('.reading-content .review-added pre code').textContent.trim(), 'new()');
  assert.match(document.querySelector('.reading-content').textContent, /Keep link\./); assert.doesNotMatch(document.querySelector('.reading-content').textContent, /\[ref\]/);
  assert.equal(document.querySelectorAll('aside .review-dock-actions').length, 0);
  assert.equal(accept().disabled, true); assert.equal(decisions.length, 0);
  await React.act(async () => document.querySelector('[aria-label="下一处修改"]').click()); assert.ok(scrolls > 0);
  await React.act(async () => [...document.querySelectorAll('.review-file-link')][1].click()); assert.deepEqual(opened, ['doc_b']); assert.equal(decisions.length, 0);
  dirty = false; await render({ documentId: 'doc_b' });
  await React.act(async () => accept().click()); assert.equal(decisions[0].id, 'proposal_b'); assert.deepEqual(refreshed, ['doc_b']);
  assert.equal(document.querySelector('.review-dock'), null); assert.equal(document.querySelector('.inline-edit-review'), null); assert.match(notices.at(-1), /已保存/);
  await render(); await React.act(async () => button('拒绝').click());
  assert.equal(decisions[1].id, 'proposal_a'); assert.equal(decisions[1].accept, false); assert.deepEqual(refreshed, ['doc_b']); assert.equal(document.querySelector('.review-dock'), null); assert.equal(document.querySelector('.review-file-links'), null);
  visible = [a]; loaded = false; await render({}, true);
  await React.act(async () => accept().click()); assert.match(document.body.textContent, /未保存的本机草稿/); assert.equal(decisions.length, 2);
  draftFails = true; await React.act(async () => accept().click()); assert.match(document.body.textContent, /草稿读取失败/); assert.equal(decisions.length, 2);
  loaded = true; draftFails = false; visible = [{ ...a, status: 'accepting' }]; await render({}, true);
  assert.equal(accept(), undefined); await React.act(async () => button('核对写入结果').click()); assert.equal(decisions.length, 3); assert.deepEqual(refreshed, ['doc_b', 'doc_a']);
  for (const [original, content] of [['', 'Only added'], ['Only removed', ''], ['```js\na()\n```', '```js\nb()\n```']]) {
   visible = [{ ...a, original, content }]; await render({}, true);
   assert.equal(document.querySelectorAll('.review-added').length > 0, !!content);
   assert.equal(document.querySelectorAll('.review-removed').length > 0, !!original);
  }
  visible = [{ ...a, original: '', content: '# Added heading\n\nNew paragraph\n\n```js\nnew()\n```\n\nLast paragraph' }]; await render({}, true);
  assert.equal(document.querySelectorAll('.review-added').length, 1);
  assert.equal(document.querySelectorAll('.review-change-label').length, 1);
  assert.equal(document.querySelectorAll('.review-source').length, 0);
  assert.equal(document.querySelector('.review-added h1').textContent, 'Added heading');
  assert.equal(document.querySelector('.review-added pre code').textContent.trim(), 'new()');
  for (const [original, content] of [
   ['[x]: https://old.example', '[x]: https://new.example'],
   ['<!-- old hidden note -->', '<!-- new hidden note -->'],
   ['Before <span data-value="old">same</span> after', 'Before <span data-value="new">same</span> after'],
   ['[same](https://old.example)', '[same](https://new.example)'],
   ['```html\n<script>window.reviewExecuted=1</script>\n```', '```html\n<script>window.reviewExecuted=2</script>\n```'],
   ['*same*', '_same_'],
  ]) {
   visible = [{ ...a, original, content }]; await render({}, true);
   assert.equal(document.querySelector('.review-removed .review-source code').textContent, original);
   assert.equal(document.querySelector('.review-added .review-source code').textContent, content);
   assert.equal(document.querySelectorAll('.review-change script,.review-change iframe,.review-change [data-value]').length, 0);
   assert.equal(window.reviewExecuted, undefined);
  }
  visible = []; await React.act(async () => { await new Promise(r => setTimeout(r, 1100)); });
  assert.equal(document.querySelector('.review-dock'), null); assert.equal(document.querySelector('.review-file-links'), null);
 } finally { await React.act(async () => root.unmount()); dom.window.close(); delete globalThis.reviewCall; await unlink(filename).catch(() => {}); }
});
