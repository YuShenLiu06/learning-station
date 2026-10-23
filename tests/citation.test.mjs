import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { JSDOM } from 'jsdom';
import { unlink } from 'node:fs/promises';
const dom = new JSDOM('<div id="body"></div>');
Object.assign(globalThis, { window: dom.window, document: dom.window.document, Node: dom.window.Node });
const file = new URL(`./.citation-${process.pid}.mjs`, import.meta.url);
await build({ stdin: { contents: `export * from './src/citation';export * from './src/renderedCitation';export {sanitizeMarkdown} from './src/sanitize';export {freezeRequestedContext,readRequestedContext} from './src/requestedContext';`, resolveDir: process.cwd(), loader: 'ts' }, outfile: file.pathname, bundle: true, packages: 'external', platform: 'node', format: 'esm' });
const ui = await import(file.href), body = document.getElementById('body');
function quote(source, target, start, end) {
    body.innerHTML = ui.sanitizeMarkdown(source);
    const element = target(body), range = document.createRange();
    if (start === undefined)
        range.selectNodeContents(element);
    else {
        range.setStart(element, start);
        range.setEnd(element, end);
    }
    return ui.renderedCitationRange(body, source, range);
}
test('rendered repeated text uses the selected occurrence, excluding Markdown URL and code language', () => {
    for (const source of ['重复句。\n\n重复句。', '[重复句。](https://fixture.invalid/重复句。)\n\n重复句。', '前文🙂\n\n**重复句。**\n\n重复句。']) {
        const value = quote(source, el => el.querySelectorAll('p')[el.querySelectorAll('p').length - 1]);
        assert.equal(value.selection, '重复句。');
        assert.equal(value.locator.startChar, Array.from(source.slice(0, source.lastIndexOf('重复句。'))).length);
    }
    const source = '```text\ntext\n```';
    assert.equal(quote(source, el => el.querySelector('code').firstChild, 0, 4).locator.startChar, 8);
});
test('formatting is exact raw source, with Unicode scalar offsets and no surrounding paragraph', () => {
    const source = '前文🙂\n\n**选中的中文**与后文🙂。\n\n不要附带此段。';
    const value = quote(source, el => el.querySelector('strong'));
    assert.deepEqual(value, { selection: '**选中的中文**', locator: { startChar: 5, endChar: 14 } });
    assert.equal(Array.from(source).slice(value.locator.startChar, value.locator.endChar).join(''), value.selection);
    assert.throws(() => quote('显示 &amp; 原文', el => el.querySelector('p')), /精确/);
    assert.throws(() => quote('正文<!--秘密-->\n\n另段', el => el.querySelector('p')), /精确/);
});
test('all three frozen source types retain their own locator and reject malformed metadata', () => {
    const locator = { startChar: 4, endChar: 6 }, selection = '原句';
    const material = { documentId: 'document', expectedVersion: 'document-version', selection, locator };
    const assistant = { sessionId: 'session', entryId: 'answer', textHash: 'a'.repeat(64), selection, locator };
    const exercise = { ...assistant, documentId: 'exercise', exerciseEntryId: 'entry', questionId: 'question', part: 'question' };
    const frozen = ui.freezeRequestedContext('document', '', [material], [assistant], [exercise]);
    locator.startChar = 88;
    for (const items of [frozen.injections, frozen.assistantReferences, frozen.exerciseReferences])
        assert.equal(items[0].locator.startChar, 4);
    assert.deepEqual(ui.readRequestedContext(frozen), frozen);
    for (const key of ['injections', 'assistantReferences', 'exerciseReferences']) {
        const invalid = structuredClone(frozen);
        invalid[key][0].locator.endChar = -1;
        assert.equal(ui.readRequestedContext(invalid), undefined);
    }
    assert.throws(() => ui.locateCitation('原句', '伪造'), /精确/);
    assert.throws(() => ui.validateCitation('🙂'.repeat(8000)), /30000/);
});
test.after(async () => { dom.window.close(); await unlink(file).catch(() => { }); });
