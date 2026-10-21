import { useEffect, useRef, useState } from 'react';
import { call } from './api';
import { emptyDebug, requestDiff, requestText } from './requestDebug';
import type { DebugSnapshot } from './requestDebug';
export function RequestDebugSettings() {
  const revision=useRef(0); const toggling=useRef(false);
  const [snapshot,setSnapshot]=useState<DebugSnapshot>(emptyDebug);
  const [session,setSession]=useState(''); const [selected,setSelected]=useState(0);
  const [tab,setTab]=useState<'text'|'payload'|'diff'>('text');
  const [error,setError]=useState(''); const [busy,setBusy]=useState(false);
  useEffect(()=>{let active=true;let pending=false;
    const refresh=async()=>{if(pending||toggling.current)return;pending=true;const version=revision.current;try{const next=await call<DebugSnapshot>('request_debug_get');if(active&&version===revision.current)setSnapshot(next);}catch{if(active)setError('请求调试状态读取失败，请重新打开设置。');}finally{pending=false;}};
    void refresh();const timer=setInterval(()=>{void refresh();},750);
    return()=>{active=false;clearInterval(timer);};
  },[]);
  async function toggle() {if(toggling.current)return;toggling.current=true;revision.current++;setBusy(true);setError('');try {setSnapshot(await call<DebugSnapshot>('request_debug_set',{enabled:!snapshot.enabled}));setSelected(0);}catch{setError('调试开关未能更新，请重试。');}finally{toggling.current=false;setBusy(false);}}
  const sessions=[...new Set(snapshot.records.map(r=>r.identity.sessionId))];
  const activeSession=sessions.includes(session)?session:sessions.at(-1)||'';
  const records=snapshot.records.filter(r=>r.identity.sessionId===activeSession);
  const record=records.find(r=>r.sequence===selected)||records.at(-1);
  const previous=record&&records.find(r=>r.sequence===record.previousSequence);
  return <section className="request-debug" aria-label="请求调试"><h2 className="settings-section-title">请求调试</h2>
    <p>查看实际发送边界捕获的模型请求，核对 Focus 正文与 diff、引用补充和 Changes。认证字段始终脱敏。</p>
    <label className="debug-switch"><input type="checkbox" checked={snapshot.enabled} disabled={busy} onChange={()=>{void toggle();}}/>开启请求调试</label>
    <p className="muted small">默认关闭。仅保存在本机易失内存，最多 40 条 / 4 MiB，单条请求体上限 256 KiB。关闭或退出应用清空；离开会话清除该会话记录。无导出与自动上传。</p>
    {error&&<p role="alert" className="inline-error">{error}</p>}
    {!snapshot.enabled?<p role="status">已关闭，不采集请求内容。</p>:<>
      <p role="status">正在采集实际请求。已淘汰或超限未保留 {snapshot.dropped} 条。</p>
      {!record?<p>尚无实际请求。返回对话发送问题，再到此查看。预览或发送前校验失败不会生成记录。</p>:<>
        <label>会话<select aria-label="调试会话" value={activeSession} onChange={e=>{setSession(e.target.value);setSelected(0);}}>{sessions.map(id=><option key={id}>{id}</option>)}</select></label>
        <label>实际请求<select aria-label="实际请求编号" value={record.sequence} onChange={e=>setSelected(Number(e.target.value))}>{records.map(r=><option key={r.sequence} value={r.sequence}>#{r.sequence} · {r.phase==='tool_continuation'?'工具后续':r.phase==='initial'?(r.identity.retry?'重试 · 本次尝试首次':'本次尝试首次'):'后续 / 压缩 / 校正'} · {r.status}</option>)}</select></label>
        <p><strong>请求 #{record.sequence}</strong> · {record.complete?'完整 payload（认证已脱敏）':`不完整：${record.omitted}`} · {record.bodyBytes} 字节</p>
        <p className="small">{record.time} · {record.status} · 运行结果：{record.runOutcome||'进行中'}</p>
        <dl className="debug-identity">{Object.entries(record.identity).map(([key,value])=><div key={key}><dt>{key}</dt><dd>{value??'未提供'}</dd></div>)}</dl>
        <div className="debug-tabs" role="group" aria-label="请求视图">{([{id:'text',label:'上下文正文'},{id:'payload',label:'完整请求'},{id:'diff',label:`diff · #${record.previousSequence||'无'} → #${record.sequence}`}] as const).map(item=><button key={item.id} className="quiet-button" aria-pressed={tab===item.id} onClick={()=>setTab(item.id)}>{item.label}</button>)}</div>
        <pre tabIndex={0} aria-label="实际请求内容">{tab==='diff'?requestDiff(previous,record):tab==='payload'?JSON.stringify({url:record.url,method:record.method,headers:record.headers,body:record.body},null,2):requestText(record)}</pre>
      </>}
    </>}
  </section>;
}
