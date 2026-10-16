import { BookOpen, ChevronDown, ChevronRight, Compass, FileText, Folder, FolderOpen, ListChecks } from 'lucide-react';
import { useMemo, useRef } from 'react';
import { visibleLibraryRows } from './treePolicy';
import type { LibraryNode } from './types';
export function LibraryTree({ nodes, activeId, expandedIds, filter, dirtyIds, onOpen, onExpanded }: { nodes: LibraryNode[]; activeId: string; expandedIds: string[]; filter: string; dirtyIds: string[]; onOpen: (id: string) => void; onExpanded: (ids: string[]) => void }) {
  const rows = useMemo(() => visibleLibraryRows(nodes, expandedIds, filter), [nodes, expandedIds, filter]);
  const refs = useRef(new Map<string, HTMLButtonElement>());
  function toggle(id: string, open?: boolean) { const expanded = new Set(expandedIds); const next = open ?? !expanded.has(id); if (next) expanded.add(id); else expanded.delete(id); onExpanded([...expanded]); }
  const focus = (id?: string) => { if (id) refs.current.get(id)?.focus(); };
  return <div className="library-tree" role="tree" aria-label="课程与学习文件">{rows.map((row, index) => {
    const node = row.node; const container = node.kind === 'course' || node.kind === 'folder'; const Icon = node.kind === 'course' ? BookOpen : node.kind === 'folder' ? row.expanded ? FolderOpen : Folder : node.kind === 'exploration' ? Compass : node.kind === 'exercise' ? ListChecks : FileText;
    return <button key={node.id} ref={element => { if (element) refs.current.set(node.id, element); else refs.current.delete(node.id); }} className={`tree-row ${activeId === node.id ? 'active' : ''}`} role="treeitem" aria-level={row.depth + 1} aria-selected={activeId === node.id} aria-expanded={row.hasChildren || container ? row.expanded : undefined} title={node.title} style={{ '--tree-depth': row.depth } as React.CSSProperties} tabIndex={node.id === activeId || !rows.some(item => item.node.id === activeId) && index === 0 ? 0 : -1} onClick={event => { if ((event.target as HTMLElement).closest('[data-tree-toggle]') && (row.hasChildren || container)) toggle(node.id); else { if (container && !row.expanded) toggle(node.id, true); onOpen(node.id); } }} onKeyDown={event => {
      if (event.key === 'ArrowDown') { event.preventDefault(); focus(rows[index + 1]?.node.id); }
      else if (event.key === 'ArrowUp') { event.preventDefault(); focus(rows[index - 1]?.node.id); }
      else if (event.key === 'Home') { event.preventDefault(); focus(rows[0]?.node.id); }
      else if (event.key === 'End') { event.preventDefault(); focus(rows.at(-1)?.node.id); }
      else if (event.key === 'ArrowRight') { event.preventDefault(); if (!row.expanded && (row.hasChildren || container)) toggle(node.id, true); else if (rows[index + 1]?.parentId === node.id) focus(rows[index + 1].node.id); }
      else if (event.key === 'ArrowLeft') { event.preventDefault(); if (row.expanded && (row.hasChildren || container)) toggle(node.id, false); else focus(row.parentId || undefined); }
    }}><span className="tree-chevron" data-tree-toggle>{row.hasChildren || container ? row.expanded ? <ChevronDown size={12}/> : <ChevronRight size={12}/> : null}</span><Icon size={14}/><span className="tree-title">{node.title}</span>{dirtyIds.includes(node.id) && <span className="draft-dot" aria-label="有未保存草稿"/>}</button>;
  })}{!rows.length && <p className="sidebar-empty">{filter ? '没有匹配的学习文件' : '先新建课程或探索，也可以打开已有材料。'}</p>}</div>;
}
