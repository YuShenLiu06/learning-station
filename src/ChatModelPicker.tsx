import { useEffect, useRef, useState } from 'react';
import { Check, ChevronDown, LoaderCircle, Settings2, X } from 'lucide-react';
import { modelFailureMessage } from './modelPolicy';
import type { ModelSettings } from './types';

export function ChatModelPicker({ model, busy, onApply, onSettings }: { model: ModelSettings; busy: boolean; onApply: (name: string) => Promise<void>; onSettings: () => void }) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(model.model);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const [error, setError] = useState('');
  useEffect(() => { setName(model.model); }, [model.model]);
  async function apply(event: React.FormEvent) {
    event.preventDefault(); if (busy || savingRef.current) return;
    savingRef.current = true; setSaving(true); setError('');
    try { await onApply(name); setEditing(false); }
    catch (error) { setError(modelFailureMessage(error, 'settings')); }
    finally { savingRef.current = false; setSaving(false); }
  }
  return <div className="chat-model-picker">
    <div className="chat-model-bar"><button className="chat-model-current" disabled={busy} aria-expanded={editing} title={busy ? '请先停止生成，再更换模型' : '更换当前供应商的模型'} onClick={() => { if (!model.configured) { onSettings(); return; } setName(model.model); setError(''); setEditing(!editing); }}><span>模型</span><strong>{model.model || '选择模型'}</strong><ChevronDown size={12} /></button><button className="icon-button" aria-label="供应商设置" title="供应商设置" onClick={onSettings}><Settings2 size={13} /></button></div>
    {editing && <form className="chat-model-form" onSubmit={event => { void apply(event); }}><label>模型 ID<input aria-label="对话模型 ID" value={name} disabled={busy || saving} onChange={event => setName(event.target.value)} autoComplete="off" spellCheck={false} placeholder="填写此供应商支持的模型 ID" /></label><div className="button-row"><button className="quiet-button" type="button" disabled={saving} onClick={() => setEditing(false)}><X size={12} />取消</button><button className="primary-button" disabled={busy || saving || !name.trim()}>{saving ? <LoaderCircle size={12} className="spin" /> : <Check size={12} />}应用模型</button></div><p className="muted small">复用已保存的供应商连接，切换后用于下一次提问。</p>{busy && !saving && <p className="muted small">请先停止生成，再更换模型。</p>}{error && <div className="inline-error" role="alert">{error}</div>}</form>}
  </div>;
}
