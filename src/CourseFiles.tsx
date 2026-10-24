import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronRight, FileText, Folder, RefreshCw, X } from 'lucide-react';
import { call, nativeAvailable } from './api';
import type { LibraryNode } from './types';
type CourseFile = { id: string; path: string; name: string; directory: boolean; parentId?: string; documentId?: string };
export function CourseFiles({ onRefreshNodes, courseId, nodes, activeId, expandedIds, onExpanded, onOpen, filter, renderDocument, renderRemaining }: { onRefreshNodes?: () => Promise<unknown>; courseId: string; nodes: LibraryNode[]; activeId: string; expandedIds: string[]; onExpanded: (ids: string[]) => void; onOpen: (id: string) => void; filter: string; renderDocument: (id: string) => React.ReactNode; renderRemaining: (displayed: string[]) => React.ReactNode }) {
  const [files, setFiles] = useState<CourseFile[]>([]); const [error, setError] = useState(''); const [loading, setLoading] = useState(false); const [revision, setRevision] = useState(0);
  const [selected, setSelected] = useState<CourseFile | null>(null); const opener = useRef<HTMLElement | null>(null);
  const close = () => { setSelected(null); opener.current?.focus(); };
  const tree = useRef<HTMLDivElement>(null); const focused = useRef<HTMLElement | null>(null); const revealed = useRef(''); const navigation = useRef('');
  const latest = useRef({onRefreshNodes, onExpanded, expandedIds}); latest.current = {onRefreshNodes, onExpanded, expandedIds};
  // A stable content signature observes in-app changes without making refreshed object identities loop.
  const nodesKey = JSON.stringify(nodes.map(n => [n.id,n.path,n.version,n.title,n.parentId]));
  useEffect(() => {
    let live = true; setFiles([]); setSelected(null); setError('');
    if (!nativeAvailable) return;
    setLoading(true);
    call<CourseFile[]>('list_course_files', { courseId }).then(async value => {
      if (!live) return;
      setFiles(value);
      // Removals and metadata edits need the same reconciliation as additions.
      await latest.current.onRefreshNodes?.();
    }).catch(e => { if (live) setError(e.message || String(e)); }).finally(() => { if (live) setLoading(false); });
    return () => { live = false; };
  }, [courseId, nodesKey, revision]);
  useEffect(() => {
    const current = `${courseId}:${activeId}`; if (navigation.current !== current) { navigation.current = current; revealed.current = ''; }
    const selected = files.find(file => file.documentId === activeId); if (!selected) return;
    const parents: string[] = []; const seen = new Set<string>(); let parent = selected.parentId;
    while (parent && !seen.has(parent)) { seen.add(parent); parents.push(parent); parent = files.find(file => file.id === parent)?.parentId; }
    const key = JSON.stringify([courseId,activeId,parents]); if (revealed.current === key) return; revealed.current = key;
    const saved = latest.current.expandedIds;
    if (parents.some(id => !saved.includes(id))) latest.current.onExpanded([...new Set([...saved,...parents])]);
  }, [courseId, activeId, files]);
  const treeRows = () => Array.from(tree.current?.querySelectorAll<HTMLButtonElement>('.tree-row') || []);
  useLayoutEffect(() => {
    const rows=treeRows(); const target=rows.find(row=>row===focused.current) || rows.find(row=>row.getAttribute('aria-selected')==='true') || rows[0];
    rows.forEach(row=>{row.tabIndex=row===target?0:-1;});
  });
  useEffect(() => { const refresh = () => setRevision(value => value + 1); window.addEventListener('focus', refresh); return () => window.removeEventListener('focus', refresh); }, []);
  const toggle = (id: string) => onExpanded(expandedIds.includes(id) ? expandedIds.filter(v => v !== id) : [...expandedIds, id]);
  const matches = new Set<string>();
  for (const file of files) if (`${file.name} ${nodes.find(n => n.id === file.documentId)?.title || ''}`.toLocaleLowerCase().includes(filter.toLocaleLowerCase())) {
    let item: CourseFile | undefined = file;
    while (item) { matches.add(item.id); item = files.find(f => f.id === item?.parentId); }
  }
  function rows(parentId?: string, depth = 0): React.ReactNode {
    return files.filter(f => (f.parentId || undefined) === parentId && matches.has(f.id)).map(file => {
      const expanded = !!filter || expandedIds.includes(file.id); const doc = nodes.find(n => n.id === file.documentId);
      if (file.documentId && doc) return <div key={file.id} style={{ paddingLeft: depth * 12 }}>{renderDocument(file.documentId)}</div>;
      return <div key={file.id}><button className={`tree-row ${file.documentId === activeId ? 'active' : ''}`} role="treeitem" aria-level={depth+1} aria-selected={file.documentId === activeId} style={{ '--tree-depth': depth } as React.CSSProperties} aria-expanded={file.directory ? expanded : undefined} title={file.path} onClick={event => { if (file.directory) toggle(file.id); else if (file.documentId) onOpen(file.documentId); else { opener.current=event.currentTarget; setSelected(file); } }}><span className="tree-chevron">{file.directory ? expanded ? <ChevronDown size={12}/> : <ChevronRight size={12}/> : null}</span>{file.directory ? <Folder size={14}/> : <FileText size={14}/>}<span className="tree-title">{doc?.title || file.name}</span></button>{file.directory && expanded && rows(file.id, depth + 1)}</div>;
    });
  }
  return <><div className="course-file-tools"><span>{loading ? '正在读取课程文件…' : '保留原有文件夹结构'}</span><button aria-label="刷新课程文件" title="刷新课程文件" onClick={() => setRevision(v => v + 1)}><RefreshCw size={13}/></button></div>{error && <p className="sidebar-status" role="alert">{error}</p>}{!nativeAvailable && <p className="sidebar-status">请在桌面应用中查看本机文件。</p>}<div ref={tree} role="tree" aria-label="课程中的实际文件" onFocusCapture={event=>{const row=(event.target as HTMLElement).closest<HTMLButtonElement>('.tree-row');if(row){focused.current=row;treeRows().forEach(item=>{item.tabIndex=item===row?0:-1;});}}} onKeyDownCapture={event=>{
    if(!['ArrowUp','ArrowDown','Home','End'].includes(event.key))return;
    const current=(event.target as HTMLElement).closest('.tree-row');if(!current)return;
    const visible=treeRows();const index=visible.findIndex(row=>row===current);if(index<0)return;
    event.preventDefault();event.stopPropagation();
    const next=event.key==='Home'?0:event.key==='End'?visible.length-1:Math.max(0,Math.min(visible.length-1,index+(event.key==='ArrowDown'?1:-1)));
    visible[next]?.focus();
  }}>{rows()}{renderRemaining(files.flatMap(f => f.documentId ? [f.documentId] : []))}</div>{selected && <div className="modal-backdrop" onKeyDown={event => { if (event.key === 'Escape') close(); if (event.key === 'Tab') { const buttons=event.currentTarget.querySelectorAll('button'); const first=buttons[0]; const last=buttons[buttons.length-1]; if (event.shiftKey && document.activeElement===first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement===last) { event.preventDefault(); first.focus(); } } }}><section className="course-file-dialog" role="dialog" aria-modal="true" aria-label={selected.name}><button autoFocus className="course-file-close" aria-label="关闭文件信息" onClick={close}><X size={18}/></button><FileText size={38}/><h2>{selected.name}</h2><p>{selected.path}</p><p>这个文件暂不支持在学习站内预览。可以在 Finder 中找到它，用合适的应用打开。</p><button className="primary-button" onClick={() => { void call('reveal_course_file', { courseId, path: selected.path }).catch(e => { setError(e.message || String(e)); close(); }); }}>在 Finder 中显示</button></section></div>}</>;
}
