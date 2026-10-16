import { useEffect, useRef, useState } from 'react';
import { call } from './api';
import type { GradingSettings as Settings } from './types';
export function GradingSettings({ blocked }: { blocked:boolean }) {
 const [settings,setSettings]=useState<Settings|null>(null),[name,setName]=useState(''),[error,setError]=useState(''),[result,setResult]=useState(''),[busy,setBusy]=useState(false);const lock=useRef(false);
 async function load(){try{const value=await call<Settings>('get_grading_settings');setSettings(value);setName(value.model||'');setError('');}catch(error){setError(error instanceof Error?error.message:String(error));}}
 useEffect(()=>{void load();},[]);
 async function save(event:React.FormEvent){event.preventDefault();if(lock.current||blocked||!settings)return;lock.current=true;setBusy(true);setError('');setResult('');try{const value=await call<Settings>('set_grading_model',{model:name.trim()||null});setSettings(value);setName(value.model||'');setResult('批改模型已保存，未发送模型请求。');}catch(error){setError(error instanceof Error?error.message:String(error));}finally{lock.current=false;setBusy(false);}}
 return <section className="grading-settings"><h2>主观题批改</h2><p className="muted small">复用当前供应商、地址与凭据。只填写批改模型 ID；留空沿用聊天模型。</p><form onSubmit={event=>{void save(event);}}><label>批改模型<input aria-label="批改模型" value={name} onChange={event=>setName(event.target.value)} placeholder="留空沿用聊天模型" maxLength={256} disabled={!settings||busy||blocked}/></label>{settings&&<p className="muted small">当前有效模型：{settings.effectiveModel||'尚未配置'}</p>}<button className="quiet-button" disabled={!settings||busy||blocked}>{busy?'正在保存…':'保存批改模型'}</button></form>{error&&<div className="inline-error" role="alert">{error}{!settings&&<button onClick={()=>{void load();}}>重试读取</button>}</div>}{result&&<p role="status">{result}</p>}</section>;
}
