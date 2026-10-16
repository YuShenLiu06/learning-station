import { useRef, useState } from 'react';
import { LoaderCircle } from 'lucide-react';
import { call } from './api';
import type { ModelSettings } from './types';

export function modelLimitsInput(output: string, context: string) {
 if (!/^\d+$/u.test(output.trim())) throw new Error('输出上限须为 1–131072 的整数。');
 const maxOutputTokens = Number(output.trim());
 if (!Number.isSafeInteger(maxOutputTokens) || maxOutputTokens < 1 || maxOutputTokens > 131072) throw new Error('输出上限须为 1–131072 的整数。');
 let contextWindow: number | null = null;
 if (context.trim()) {
  if (!/^\d+$/u.test(context.trim())) throw new Error('上下文窗口须留空或填写 1024–2000000 的整数。');
  contextWindow = Number(context.trim());
  if (!Number.isSafeInteger(contextWindow) || contextWindow < 1024 || contextWindow > 2000000) throw new Error('上下文窗口须留空或填写 1024–2000000 的整数。');
  if (maxOutputTokens > contextWindow - 1024) throw new Error('请至少为输入预留 1024 token；输出上限不能超过上下文窗口减 1024。');
 }
 return { maxOutputTokens, contextWindow };
}
export function ModelLimitsSettings({ model, blocked, unsavedConnection, onSaved }: { model: ModelSettings; blocked: boolean; unsavedConnection: boolean; onSaved: (value: ModelSettings) => void }) {
 const [output, setOutput] = useState(typeof model.maxOutputTokens === 'number' ? String(model.maxOutputTokens) : '');
 const [context, setContext] = useState(typeof model.contextWindow === 'number' ? String(model.contextWindow) : '');
 const [busy, setBusy] = useState<'save' | 'read' | ''>('');
 const [error, setError] = useState(''), [result, setResult] = useState('');
 const lock = useRef(false);
 const hasOutput = Number.isInteger(model.maxOutputTokens) && model.maxOutputTokens! >= 1 && model.maxOutputTokens! <= 131072;
 const unavailable = !model.configured || blocked || unsavedConnection;
 function apply(value: ModelSettings) { onSaved(value); if (typeof value.maxOutputTokens === 'number') setOutput(String(value.maxOutputTokens)); setContext(typeof value.contextWindow === 'number' ? String(value.contextWindow) : ''); }
 async function read() { if (lock.current || unavailable) return; lock.current = true; setBusy('read'); setError(''); setResult(''); try { const value = await call<ModelSettings>('model_get'); apply(value); if (typeof value.maxOutputTokens !== 'number') setError('本机服务尚未返回输出上限，请核对应用版本后重新读取。'); } catch (error) { setError(`限额尚未读取，当前输入保留：${error instanceof Error ? error.message : String(error)}`); } finally { lock.current = false; setBusy(''); } }
 async function save(event: React.FormEvent) {
  event.preventDefault(); if (lock.current || unavailable || !hasOutput) return;
  setError(''); setResult(''); let input: ReturnType<typeof modelLimitsInput>;
  try { input = modelLimitsInput(output, context); } catch (error) { setError(error instanceof Error ? error.message : String(error)); return; }
  lock.current = true; setBusy('save');
  try { apply(await call<ModelSettings>('model_limits_save', input)); setResult('限额已保存到本机，后续生成和主观题批改使用此预算。'); }
  catch (error) { setError(`限额尚未保存，当前输入保留：${error instanceof Error ? error.message : String(error)}`); }
  finally { lock.current = false; setBusy(''); }
 }
 return <section className="settings-local" aria-label="模型生成限额"><h3>生成限额</h3><p className="muted small">用于当前已保存连接。输出上限可能包含思考 token，实际能力取决于所选服务。更换同一连接的模型会保留预算，请核对新模型支持的范围。</p><p className="muted small">当前输出上限：{hasOutput ? model.maxOutputTokens : '尚未读取'} · 上下文窗口：{model.contextWindow === null ? '未知（未声明）' : typeof model.contextWindow === 'number' ? model.contextWindow : '尚未读取'}</p>
  {!model.configured ? <p role="status">请先保存供应商连接，再设置生成限额。</p> : unsavedConnection ? <p role="status">连接有未保存修改，请先保存或还原连接，再修改已保存连接的限额。</p> : blocked ? <p role="status">正在生成、批改或更新连接，请结束当前操作后保存限额。</p> : null}
  {!hasOutput && <p role="status">输出上限尚未从本机服务读取，不使用推测值。<button type="button" className="quiet-button" disabled={unavailable || !!busy} onClick={()=>{void read();}}>{busy === 'read' ? '正在读取限额…' : '重新读取限额'}</button></p>}
  <form noValidate onSubmit={event=>{void save(event);}}><fieldset className="provider-fields" disabled={unavailable || !!busy || !hasOutput}><label>最大输出 token<input aria-label="最大输出 token" inputMode="numeric" value={output} onChange={event=>{setOutput(event.target.value);setError('');setResult('');}} placeholder="从本机服务读取后填写"/><span className="label-note">1–131072 的整数；不会自动调整为最大值。</span></label><label>上下文窗口（可选）<input aria-label="上下文窗口" inputMode="numeric" value={context} onChange={event=>{setContext(event.target.value);setError('');setResult('');}} placeholder="未知请留空"/><span className="label-note">只填写服务方确认的窗口大小；留空表示未知。至少 1024 的输入预留是配置约束，不是精确 token 计数。</span></label></fieldset>{error && <p className="inline-error" role="alert">{error}</p>}{result && <p className="settings-result" role="status">{result}</p>}<div className="dialog-footer"><button className="primary-button" disabled={unavailable || !!busy || !hasOutput}>{busy === 'save' ? <><LoaderCircle size={14} className="spin"/>正在保存限额…</> : '保存生成限额'}</button></div></form>
 </section>;
}
