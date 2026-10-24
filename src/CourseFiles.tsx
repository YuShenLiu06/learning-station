import { useEffect, useRef, useState } from 'react';
import { ChevronDown, ChevronRight, FileText, Folder, RefreshCw, X } from 'lucide-react';
import { call, nativeAvailable } from './api';
import type { LibraryNode } from './types';
type CourseFile = { id: string; path: string; name: string; directory: boolean; parentId?: string; documentId?: string };
export function CourseFiles({ courseId, nodes, activeId, expandedIds, onExpanded, onOpen, filter, renderDocument, renderRemaining }: { courseId: string; nodes: LibraryNode[]; activeId: string; expandedIds: string[]; onExpanded: (ids: string[]) => void; onOpen: (id: string) => void; filter: string; renderDocument: (id: string) => React.ReactNode; renderRemaining: (displayed: string[]) => React.ReactNode }) {
  const [files, setFiles] = useState<CourseFile[]>([]); const [error, setError] = useState(''); const [loading, setLoading] = useState(false); const [revision, setRevision] = useState(0);
  const [selected, setSelected] = useState<CourseFile | null>(null); const opener = useRef<HTMLElement | null>(null);
  const close = () => { setSelected(null); opener.current?.focus(); };
  useEffect(() => {
    let live = true; setFiles([]); setSelected(null); setError('');
    if (!nativeAvailable) return;
    setLoading(true);
    call<CourseFile[]>('list_course_files', { courseId }).then(value => { if (live) setFiles(value); }).catch(e => { if (live) setError(e.message || String(e)); }).finally(() => { if (live) setLoading(false); });
    return () => { live = false; };
  }, [courseId, nodes, revision]);
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
      if (file.documentId) return <div key={file.id} style={{ paddingLeft: depth * 12 }}>{renderDocument(file.documentId)}</div>;
      return <div key={file.id}><button className={`tree-row ${file.documentId === activeId ? 'active' : ''}`} style={{ '--tree-depth': depth } as React.CSSProperties} aria-expanded={file.directory ? expanded : undefined} title={file.path} onClick={event => { if (file.directory) toggle(file.id); else if (file.documentId) onOpen(file.documentId); else { opener.current=event.currentTarget; setSelected(file); } }}><span className="tree-chevron">{file.directory ? expanded ? <ChevronDown size={12}/> : <ChevronRight size={12}/> : null}</span>{file.directory ? <Folder size={14}/> : <FileText size={14}/>}<span className="tree-title">{doc?.title || file.name}</span></button>{file.directory && expanded && rows(file.id, depth + 1)}</div>;
    });
  }
  return <><div className="course-file-tools"><span>{loading ? '正在读取课程文件…' : '保留原有文件夹结构'}</span><button aria-label="刷新课程文件" title="刷新课程文件" onClick={() => setRevision(v => v + 1)}><RefreshCw size={13}/></button></div>{error && <p className="sidebar-status" role="alert">{error}</p>}{!nativeAvailable && <p className="sidebar-status">请在桌面应用中查看本机文件。</p>}<div aria-label="课程中的实际文件">{rows()}</div>{renderRemaining(files.flatMap(f => f.documentId ? [f.documentId] : []))}{selected && <div className="modal-backdrop" onKeyDown={event => { if (event.key === 'Escape') close(); if (event.key === 'Tab') { const buttons=event.currentTarget.querySelectorAll('button'); const first=buttons[0]; const last=buttons[buttons.length-1]; if (event.shiftKey && document.activeElement===first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement===last) { event.preventDefault(); first.focus(); } } }}><section className="course-file-dialog" role="dialog" aria-modal="true" aria-label={selected.name}><button autoFocus className="course-file-close" aria-label="关闭文件信息" onClick={close}><X size={18}/></button><FileText size={38}/><h2>{selected.name}</h2><p>{selected.path}</p><p>这个文件暂不支持在学习站内预览。可以在 Finder 中找到它，用合适的应用打开。</p><button className="primary-button" onClick={() => { void call('reveal_course_file', { courseId, path: selected.path }).catch(e => { setError(e.message || String(e)); close(); }); }}>在 Finder 中显示</button></section></div>}</>;
}
