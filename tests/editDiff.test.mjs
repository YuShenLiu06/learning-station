import test from 'node:test';
import assert from 'node:assert/strict';
import { editDiff } from '../src/editDiff.ts';
test('Diff reconstructs both exact texts including empty/trailing lines and code', () => {
  for (const [before, after] of [['a\nb\nc','a\nx\nc'], ['', 'new'], ['old',''], ['a\n','a'], ['```js\nx()\n```\n公式 $x$','```js\ny()\n```\n公式 $y$'], ['same','same']]) {
    const diff = editDiff(before, after);
    assert.equal(diff.filter(l => l.kind !== 'added').map(l => l.text).join('\n'), before);
    assert.equal(diff.filter(l => l.kind !== 'removed').map(l => l.text).join('\n'), after);
  }
});
test('separated edits leave intervening unchanged lines intact', () => {
  const diff = editDiff('a\nold\nkeep\nold2\nz', 'a\nnew\nkeep\nnew2\nz');
  assert.equal(diff.find(l => l.text === 'keep')?.kind, 'same');
});
