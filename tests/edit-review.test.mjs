import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';

test('review appears in place, protects dirty drafts and refreshes only after persisted acceptance', async () => {
 const dom = new JSDOM('<div id="root"></div>', { url: 'http://127.0.0.1/' });
 Object.assign(globalThis, { window: dom.window, document: dom.window.document, IS_REACT_ACT_ENVIRONMENT: true });
 const filename = new URL(`./.edit-review-${process.pid}.mjs`, import.meta.url);
 await build({ entryPoints: ['src/EditReview.tsx'], outfile: filename.pathname, bundle: true, packages: 'external', platform: 'node', format: 'esm', jsx: 'automatic', plugins: [{ name: 'isolated-review-ipc', setup(b) {
  b.onResolve({ filter: /^\.\/api$/ }, () => ({ path: 'review-ipc', namespace: 'fixture' }));
  b.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: 'export const nativeAvailable=true; export const call=(...args)=>globalThis.reviewCall(...args);', loader: 'js' }));
 }}] });
 const React = await import('react'); const { createRoot } = await import('react-dom/client');
 const { EditReview } = await import(filename.href); const root = createRoot(document.getElementById('root'));
 const proposal = { id: 'proposal_a', courseId: 'course_a', path: 'documents/doc_a.md', original: 'Old body', content: 'New body', status: 'pending' };
 let loaded = true, draftFails = false, dirty = true, decisions = [], refreshed = [], pending = true;
 globalThis.reviewCall = async (command, payload) => {
  if (command === 'edit_proposals') return pending ? [{ id: proposal.id, stamp: '1' }] : [];
  if (command === 'edit_proposal') return proposal;
  if (command === 'section_open') return { body: 'Old body' };
  if (command === 'draft_get') { if (draftFails) throw Error('草稿读取失败'); return { data: { body: 'Unopened local draft' } }; }
  if (command === 'decide_edit_proposal') { decisions.push(payload); pending = false; return payload.accept ? { persisted: true } : { persisted: false, status: 'rejected' }; }
  throw Error(command);
 };
 const props = { courseId: 'course_a', documentId: 'doc_a', isDirty: () => dirty, isLoaded: () => loaded, onAccepted: async id => refreshed.push(id) };
 const button = text => [...document.querySelectorAll('button')].find(b => b.textContent === text);
 try {
  await React.act(async () => { root.render(React.createElement(EditReview, props)); await new Promise(r => setTimeout(r, 20)); });
  assert.match(document.body.textContent, /建议已保留，正文未修改/);
  assert.match(document.querySelector('.edit-diff').textContent, /− Old body/);
  assert.match(document.querySelector('.edit-diff').textContent, /\+ New body/);
  assert.equal(button('接受修改').disabled, true); assert.equal(decisions.length, 0);
  dirty = false;
  await React.act(async () => root.render(React.createElement(EditReview, { ...props })));
  await React.act(async () => button('接受修改').click());
  assert.deepEqual(refreshed, ['doc_a']); assert.equal(decisions[0].accept, true);
  assert.match(document.body.textContent, /修改已保存到正文/);
  pending = true; dirty = true;
  await React.act(async () => root.render(React.createElement(EditReview, { ...props, key: 'reject' })));
  await React.act(async () => { await new Promise(r => setTimeout(r, 20)); });
  await React.act(async () => button('拒绝修改').click());
  assert.equal(decisions[1].accept, false); assert.deepEqual(refreshed, ['doc_a']);
  assert.match(document.body.textContent, /已拒绝本次建议，正文未修改/);
  pending = true; dirty = false; loaded = false;
  await React.act(async () => root.render(React.createElement(EditReview, { ...props, documentId: 'doc_other', key: 'unopened' })));
  await React.act(async () => { await new Promise(r => setTimeout(r, 20)); });
  const select = document.querySelector('select'); select.value = proposal.id;
  await React.act(async () => select.dispatchEvent(new dom.window.Event('change', { bubbles: true })));
  await React.act(async () => button('接受修改').click());
  assert.match(document.body.textContent, /未保存的本机草稿/); assert.equal(decisions.length, 2);
  draftFails = true;
  await React.act(async () => button('接受修改').click());
  assert.match(document.body.textContent, /草稿读取失败/); assert.equal(decisions.length, 2);
  loaded = true; draftFails = false; proposal.status = 'accepting';
  await React.act(async () => root.render(React.createElement(EditReview, { ...props, key: 'recovery' })));
  await React.act(async () => { await new Promise(r => setTimeout(r, 20)); });
  assert.equal(button('接受修改').disabled, true);
  await React.act(async () => button('核对写入结果').click());
  assert.equal(decisions.length, 3); assert.deepEqual(refreshed, ['doc_a', 'doc_a']);


 } finally { await React.act(async () => root.unmount()); dom.window.close(); delete globalThis.reviewCall; await unlink(filename).catch(() => {}); }
});
