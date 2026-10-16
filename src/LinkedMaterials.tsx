import { useState } from 'react';
import { LoaderCircle } from 'lucide-react';
import type { LibraryNode, Section } from './types';

export function LinkedMaterials({ section, nodes, onSave }: { section: Section; nodes: LibraryNode[]; onSave: (links: string[]) => Promise<void> }) {
  const [selected, setSelected] = useState(section.links || []);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const available = nodes.filter(node => node.id !== section.id && ['section', 'knowledge', 'exploration'].includes(node.kind));
  const missing = selected.filter(id => !available.some(node => node.id === id));
  async function submit(event: React.FormEvent) {
    event.preventDefault(); if (busy) return; setBusy(true); setError('');
    try { await onSave(selected); } catch (error) { setError(error instanceof Error ? error.message : String(error)); } finally { setBusy(false); }
  }
  return <form onSubmit={event => { void submit(event); }}><p className="muted small">只带入明确勾选的材料。目录归属不会自动关联整门课程；跨课材料也须在这里明确选择。解绑保留笔记正文与同一会话历史。</p><div className="linked-material-list">{available.map(node => <label className="checkbox-label" key={node.id}><input type="checkbox" checked={selected.includes(node.id)} disabled={busy || selected.length >= 16 && !selected.includes(node.id)} onChange={event => setSelected(old => event.target.checked ? [...old, node.id] : old.filter(id => id !== node.id))}/><span>{node.title}<small>{node.path}</small></span></label>)}{missing.map(id => <label className="checkbox-label" key={id}><input type="checkbox" checked disabled={busy} onChange={() => setSelected(old => old.filter(value => value !== id))}/><span>原关联材料暂不可用<small>{id}</small></span></label>)}{!available.length && !missing.length && <p className="muted">还没有可关联的学习材料；可先创建或导入 Markdown 材料。</p>}</div>{error && <p className="inline-error" role="alert">{error} 当前选择与正文均保留。</p>}<div className="dialog-footer"><button className="quiet-button" type="button" disabled={busy || !selected.length} onClick={() => setSelected([])}>全部解绑</button><span>{selected.length} / 16 份材料</span><button className="primary-button" disabled={busy}>{busy && <LoaderCircle className="spin" size={14}/>}保存关联</button></div></form>;
}
