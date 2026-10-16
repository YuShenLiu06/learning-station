import type { LibraryNode } from './types';
export type TreeRow = { node: LibraryNode; depth: number; parentId: string | null; hasChildren: boolean; expanded: boolean };
export function visibleLibraryRows(nodes: LibraryNode[], expandedIds: string[], query = ''): TreeRow[] {
  const byId = new Map(nodes.map(node => [node.id, node]));
  const children = new Map<string | null, LibraryNode[]>();
  const parent = (node: LibraryNode) => node.parentId && byId.has(node.parentId) && node.parentId !== node.id ? node.parentId : null;
  for (const node of nodes) { const key = parent(node); children.set(key, [...(children.get(key) || []), node]); }
  const compare = (a: LibraryNode, b: LibraryNode) => (a.order ?? 0) - (b.order ?? 0) || a.title.localeCompare(b.title, 'zh-CN') || a.id.localeCompare(b.id);
  for (const values of children.values()) values.sort(compare);
  const expanded = new Set(expandedIds); const term = query.trim().toLocaleLowerCase();
  const kept = new Set<string>();
  if (term) for (const node of nodes) if (node.title.toLocaleLowerCase().includes(term)) { let current: LibraryNode | undefined = node; const path = new Set<string>(); while (current && !path.has(current.id)) { path.add(current.id); kept.add(current.id); current = byId.get(parent(current) || ''); } }
  const result: TreeRow[] = []; const visited = new Set<string>();
  function visit(node: LibraryNode, depth: number) {
    if (visited.has(node.id) || term && !kept.has(node.id)) return;
    visited.add(node.id); const descendants = children.get(node.id) || [];
    const open = !!term || expanded.has(node.id);
    result.push({ node, depth, parentId: parent(node), hasChildren: descendants.length > 0, expanded: open });
    if (open) for (const child of descendants) visit(child, Math.min(depth + 1, 20));
  }
  for (const root of children.get(null) || []) visit(root, 0);
  // Malformed external metadata must stay accessible without recursing forever.
  for (const node of [...nodes].sort(compare)) if (!visited.has(node.id)) {
    let current: LibraryNode | undefined = node; const chain = new Set<string>(); let cycle = false;
    while (current) { if (chain.has(current.id)) { cycle = true; break; } chain.add(current.id); current = byId.get(parent(current) || ''); }
    if (cycle) visit(node, 0);
  }
  return result;
}
export function ancestorIds(nodes: LibraryNode[], id: string) {
  const byId = new Map(nodes.map(node => [node.id, node])); const result: string[] = []; const seen = new Set([id]); let node = byId.get(id);
  while (node?.parentId && byId.has(node.parentId) && !seen.has(node.parentId)) { result.push(node.parentId); seen.add(node.parentId); node = byId.get(node.parentId); }
  return result;
}
export function containingCourse(nodes: LibraryNode[], id: string) {
  const ids = [id, ...ancestorIds(nodes, id)]; return ids.map(value => nodes.find(node => node.id === value)).find(node => node?.kind === 'course');
}
