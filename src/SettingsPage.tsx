import { useState } from 'react';
import { ArrowLeft, Bell, FolderOpen, LoaderCircle, Palette, Settings2 } from 'lucide-react';
import { call } from './api';
import { modelAccessReason, modelAccessMessage, modelBindingChanged, modelFailureMessage, modelFormError, modelRequestTarget, supportedProtocol } from './modelPolicy';
import type { ModelSettings } from './types';
import type { useFontSize } from './useFontSize';
import { GradingSettings } from './GradingSettings';
import { FeishuSettings } from './FeishuSettings';
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

export function SettingsPage({ model, dataPath, onClose, onSaved, font, blocked }: { model: ModelSettings; dataPath?: string; onClose: () => void; onSaved: (model: ModelSettings) => void; font: ReturnType<typeof useFontSize>; blocked: boolean }) {
  const [category, setCategory] = useState<'provider' | 'appearance' | 'notifications'>('provider');
  const [provider, setProvider] = useState(model.provider === 'glm-pi' ? 'glm-pi' : 'custom');
  const [protocol, setProtocol] = useState(model.protocol || supportedProtocol);
  const [baseUrl, setBaseUrl] = useState(model.baseUrl || '');
  const [name, setName] = useState(model.model || '');
  const [secret, setSecret] = useState('');
  const [busy, setBusy] = useState<'save' | 'test' | ''>('');
  const [error, setError] = useState('');
  const [result, setResult] = useState('');
  const bindingChanged = modelBindingChanged(model, { provider, protocol, baseUrl });
  const settingsChanged = bindingChanged || name.trim() !== model.model;
  const savedTarget = model.requestUrl || modelRequestTarget({ provider: model.provider, protocol: model.protocol || supportedProtocol, baseUrl: model.baseUrl });
  const pendingTarget = modelRequestTarget({ provider, protocol, baseUrl });
  async function save(event: React.FormEvent) {
    event.preventDefault(); if (busy || blocked) return;
    setError(''); setResult('');
    const validation = modelFormError({ provider, protocol, baseUrl, model: name, apiKey: secret }, model);
    if (validation) { setError(validation); return; }
    setBusy('save'); const apiKey = secret; setSecret('');
    try { const saved = await call<ModelSettings>('model_save', { provider, protocol, baseUrl: baseUrl.trim(), model: name.trim(), apiKey: apiKey || undefined }); onSaved(saved); setBaseUrl(saved.baseUrl); setName(saved.model); setProtocol(saved.protocol || supportedProtocol); setResult(modelAccessMessage(saved) ? `配置已保存到本机。${modelAccessMessage(saved)}` : '配置已保存到本机，尚未发送模型请求。可以继续测试已保存的连接。'); }
    catch (error) { setError(`连接配置尚未保存：${errorText(error)}`); }
    finally { setBusy(''); }
  }
  async function test() {
    if (busy || blocked) return; setError(''); setResult('');
    if (!model.configured || settingsChanged) { setError(settingsChanged ? '配置有更改，请先保存，再测试这个目标。' : '请先填写并保存模型连接，再测试。'); return; }
    if (modelAccessMessage(model)) { setError(`本地用途限制：${modelAccessMessage(model)}`); return; }
    setBusy('test');
    try { const response = await call<{ ok: boolean; message: string }>('model_test'); setResult(response.message); onSaved(await call<ModelSettings>('model_get')); }
    catch (error) { setError(modelFailureMessage(error, 'test')); }
    finally { setBusy(''); }
  }
  return <section className="utility-page settings-page" aria-label="供应商与设置"><button className="text-back" onClick={onClose}><ArrowLeft size={14} />返回学习</button><span className="eyebrow">YOUR MODEL CONNECTION</span><h1>设置</h1><p className="page-description">调整自己的学习空间。连接、阅读与通知分别管理，修改时已有输入会保留。</p><div className="settings-layout"><nav className="settings-categories" role="tablist" aria-label="设置分类" aria-orientation="vertical" onKeyDown={event => { const keys = ['provider', 'appearance', 'notifications'] as const; const index = keys.indexOf(category); const next = event.key === 'ArrowDown' ? keys[(index + 1) % keys.length] : event.key === 'ArrowUp' ? keys[(index + keys.length - 1) % keys.length] : event.key === 'Home' ? keys[0] : event.key === 'End' ? keys.at(-1) : undefined; if (next) { event.preventDefault(); setCategory(next); document.getElementById(`settings-tab-${next}`)?.focus(); } }}>
      {([{id:'provider',label:'供应商',icon:Settings2},{id:'appearance',label:'外观与阅读',icon:Palette},{id:'notifications',label:'通知',icon:Bell}] as const).map(item => <button key={item.id} id={`settings-tab-${item.id}`} role="tab" aria-selected={category === item.id} aria-controls={`settings-${item.id}`} tabIndex={category === item.id ? 0 : -1} className={category === item.id ? 'selected' : ''} onClick={() => setCategory(item.id)}><item.icon size={16}/>{item.label}</button>)}
    </nav><div className="settings-panels"><section id="settings-provider" role="tabpanel" aria-labelledby="settings-tab-provider" hidden={category !== 'provider'}><h2 className="settings-section-title">模型供应商</h2><p className="muted small">连接信息保存在本机。日常更换模型可直接在对话中完成，无需重新填写供应商或密钥。</p><div className="provider-settings">

    <div className={`settings-status ${model.connected && !modelAccessReason(model) ? 'connected' : ''}`}><span className="connection-dot" />{modelAccessReason(model) ? '已保存配置 · 本地用途限制' : model.connected ? '已保存目标的真实连接已验证' : model.configured ? '已保存配置 · 连接待验证' : '模型服务尚未配置'}</div>
    {modelAccessMessage(model) && <div className="inline-error model-policy" role="status"><strong>当前保存的地址受本地限制</strong><p>{modelAccessMessage(model)}</p></div>}
    <form noValidate onSubmit={event => { void save(event); }}><fieldset className="provider-fields" disabled={!!busy || blocked}>
      <label>模型服务类型<select aria-label="模型服务类型" value={provider} onChange={event => { setProvider(event.target.value); setSecret(''); }}><option value="custom">自定义模型服务</option><option value="glm-pi">GLM Pi · Coding Plan 用途受限</option></select></label>
      {provider === 'glm-pi' && !modelAccessReason(model) && <div className="inline-error model-policy" role="status"><strong>Coding Plan 用途限制仍有效</strong><p>Pi 接入不代表套餐授权。此用途尚未确认，服务端会保留请求门槛；不会自动切换到付费 API。自定义服务请使用你获准调用的接口。</p></div>}
      <label>模型协议<select aria-label="模型协议" value={protocol} onChange={event => { setProtocol(event.target.value); setSecret(''); }}><option value={supportedProtocol}>OpenAI Chat Completions</option><option value="anthropic-messages" disabled={provider !== 'custom'}>Anthropic Messages · 自定义服务</option></select><span className="label-note">{protocol === 'anthropic-messages' ? '使用标准 API key（x-api-key），不使用 OAuth；请先确认所选服务支持此协议。' : 'OpenAI Chat Completions · openai-completions，由 Pi 适配'}</span></label>
      <label>服务地址 base_url<input aria-label="模型服务地址" type="url" value={baseUrl} onChange={event => { setBaseUrl(event.target.value); setSecret(''); }} placeholder="填写服务提供方给出的基础 base_url" spellCheck={false} autoComplete="off" /><span className="label-note">{protocol === 'anthropic-messages' ? '填写根地址或代理前缀，末尾不含 /v1 或 /messages；此版 Pi SDK 固定追加 /v1/messages?beta=true。' : '基础地址须包含服务要求的 /v1 等前缀，不含 /chat/completions；应用直接追加 /chat/completions，不自动补 /v1。'}测试也会调用所选服务，并按服务方规则计费。</span></label>
      <div className="settings-local" aria-label="模型请求目标">{model.configured && <p><strong>已保存请求目标</strong><br />{savedTarget || '当前配置尚无有效请求目标'}<br />测试与发送问题使用已保存配置。</p>}{(!model.configured || settingsChanged) && <p><strong>待保存请求目标</strong><br />{pendingTarget || '填写有效的协议与基础地址后显示'}<br />保存只写入本机；不会自动测试、切换地址或模型。</p>}</div>
      <label>默认模型<input aria-label="模型名称" value={name} onChange={event => setName(event.target.value)} placeholder="填写服务端支持的模型 ID" spellCheck={false} autoComplete="off" /><span className="label-note">保存后也可以在对话中单独更换模型。</span></label>
      <label>API 密钥 <span className="label-note">{model.credentialStored && !bindingChanged ? '已由本机安全存储管理；留空保留' : '更改地址或协议后需要新密钥'}</span><input aria-label="API 密钥" type="password" value={secret} onChange={event => setSecret(event.target.value)} placeholder={model.credentialStored && !bindingChanged ? '留空保留已有密钥' : '请在本机输入此服务的密钥'} autoComplete="new-password" spellCheck={false} /></label>
      <p className="muted small">密钥不进入网页存储或日志；提交后立即清空输入框。测试只针对已保存的地址、协议和模型。</p>
      </fieldset>{blocked && <p className="muted small">正在生成或切换模型；请先回到对话停止生成，再修改连接。</p>}{error && <div className="inline-error" role="alert">{error}</div>}{result && <p className="settings-result" role="status">{result}</p>}
      <div className="dialog-footer"><button type="button" className="quiet-button" disabled={!!busy || blocked} onClick={() => { void test(); }}>{busy === 'test' ? <><LoaderCircle size={14} className="spin" />正在测试已保存连接…</> : modelAccessReason(model) && !settingsChanged ? '查看本地用途限制' : '测试已保存连接'}</button><button className="primary-button" disabled={!!busy || blocked}>{busy === 'save' ? <><LoaderCircle size={14} className="spin" />正在保存配置…</> : '保存连接设置'}</button></div>
    </form>
    </div><GradingSettings blocked={blocked}/></section><section id="settings-notifications" role="tabpanel" aria-labelledby="settings-tab-notifications" hidden={category !== 'notifications'}><FeishuSettings /></section><section id="settings-appearance" className="appearance-settings" role="tabpanel" aria-labelledby="settings-tab-appearance" hidden={category !== 'appearance'}><h2 className="settings-section-title">外观与阅读</h2>
    <section className="font-settings" aria-label="内容字号设置"><div><strong>内容字号</strong><output aria-live="polite">{font.fontSize}px</output></div><div className="font-controls"><button className="quiet-button" aria-label="减小内容字号" disabled={font.fontSize <= 12} onClick={() => font.setFontSize(font.fontSize - 1)}>A−</button><input aria-label="内容字号" type="range" min="12" max="24" step="1" value={font.fontSize} onChange={event => font.setFontSize(Number(event.target.value))} /><button className="quiet-button" aria-label="增大内容字号" disabled={font.fontSize >= 24} onClick={() => font.setFontSize(font.fontSize + 1)}>A＋</button></div><p>阅读、编辑、对话与复习内容同步调整。{font.status}</p>{font.error && <div className="inline-error" role="alert">字号设置：{font.error}<button onClick={() => { void font.flush().catch(() => undefined); }}>重试保存字号</button><button onClick={() => { void font.retry(); }}>重新读取字号</button></div>}</section>
    <div className="settings-local"><p>Anki：在“复习”中连接本机 AnkiConnect；未连接时可降级导出 CSV</p><strong><FolderOpen size={14} />本机材料目录</strong><p>{dataPath || '由本机服务管理，连接后显示路径'}</p><p>Markdown 是学习内容的原始文件，保存时会检查版本变化。</p></div>
    </section></div></div>
  </section>;
}

