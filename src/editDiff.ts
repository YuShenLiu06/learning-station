export type DiffLine = { kind: 'same' | 'removed' | 'added'; text: string };
// LCS within a fixed memory/work budget. Common edges are stripped first;
// very large unmatched spans remain one honest, reversible changed region.
export function editDiff(before: string, after: string): DiffLine[] {
  const a = before.split('\n'), b = after.split('\n');
  let start = 0, end = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start++;
  while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end++;
  const x = a.slice(start, a.length - end), y = b.slice(start, b.length - end);
  const result: DiffLine[] = a.slice(0, start).map(text => ({ kind: 'same', text }));
  if ((x.length + 1) * (y.length + 1) <= 2_000_000) {
    const width = y.length + 1, table = new Uint32Array((x.length + 1) * width);
    for (let i = x.length - 1; i >= 0; i--) for (let j = y.length - 1; j >= 0; j--) {
      table[i * width + j] = x[i] === y[j] ? 1 + table[(i + 1) * width + j + 1] : Math.max(table[(i + 1) * width + j], table[i * width + j + 1]);
    }
    let i = 0, j = 0;
    while (i < x.length || j < y.length) {
      if (i < x.length && j < y.length && x[i] === y[j]) { result.push({ kind: 'same', text: x[i++] }); j++; }
      else if (j < y.length && (i === x.length || table[i * width + j + 1] > table[(i + 1) * width + j])) result.push({ kind: 'added', text: y[j++] });
      else result.push({ kind: 'removed', text: x[i++] });
    }
  } else {
    result.push(...x.map(text => ({ kind: 'removed' as const, text })), ...y.map(text => ({ kind: 'added' as const, text })));
  }
  result.push(...a.slice(a.length - end).map(text => ({ kind: 'same' as const, text })));
  return result;
}
