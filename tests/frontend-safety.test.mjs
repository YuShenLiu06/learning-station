import test from 'node:test';
import assert from 'node:assert/strict';
import { JSDOM } from 'jsdom';
import { isSubmitKey } from '../src/keyboard.ts';

const dom = new JSDOM('');
globalThis.window = dom.window;
const { sanitizeMarkdown } = await import('../src/sanitize.ts');
function render(source) { const element = dom.window.document.createElement('div'); element.innerHTML = sanitizeMarkdown(source); return element; }

test('untrusted Markdown keeps prose while removing scripts, event handlers, SVG and native command links', () => {
  const result = render('# 课程\n\n正文 **still readable**\n<script>alert(1)</script><img src=x onerror=alert(2)><iframe src="https://example.com"></iframe><svg onload=alert(3)></svg><a href="tauri://localhost" onclick=alert(4)>native</a>\n\n[run](javascript:alert(5))');
  assert.match(result.textContent, /课程/);
  assert.match(result.textContent, /still readable/);
  assert.equal(result.querySelectorAll('script,img,iframe,svg,a,[onerror],[onclick],[onload],[href]').length, 0);
  assert.equal(result.querySelector('strong').textContent, 'still readable');
});
test('nested malformed markup cannot expose forms or executable attributes', () => {
  const result = render('<math><mtext><table><mglyph><style><!--</style><img title="--><img src=x onerror=alert(1)>"><form action="tauri://localhost"><input autofocus onfocus=alert(2)></form>');
  for (const element of result.querySelectorAll('*')) for (const name of element.getAttributeNames()) assert.ok(!/^on|src|href|action|autofocus$/i.test(name));
  assert.equal(result.querySelectorAll('form,input,style,math').length, 0);
});
test('source URLs and external images cannot make automatic network requests', () => {
  const result = render('![remote](https://example.com/private-image)\n\n[link](https://example.com)\n\n<video poster="https://example.com/a"><source src="https://example.com/b"></video>');
  assert.equal(result.querySelectorAll('[src],[href],[poster],video,source,img').length, 0);
  assert.match(result.textContent, /link/);
});
test('plain Enter and Chinese IME composition never submit a question', () => {
  assert.equal(isSubmitKey({key:'Enter',metaKey:false,ctrlKey:false}), false);
  assert.equal(isSubmitKey({key:'Enter',metaKey:true,ctrlKey:false,isComposing:true}), false);
  assert.equal(isSubmitKey({key:'Enter',metaKey:true,ctrlKey:false,keyCode:229}), false);
  assert.equal(isSubmitKey({key:'Enter',metaKey:true,ctrlKey:false,isComposing:false,keyCode:13}), true);
  assert.equal(isSubmitKey({key:'Enter',metaKey:false,ctrlKey:true}), true);
});
