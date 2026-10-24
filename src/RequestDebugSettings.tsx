import { useEffect, useRef, useState } from 'react';
import { ChevronRight, Download } from 'lucide-react';
import { call } from './api';
import { DebugJsonView } from './DebugJsonView';
import { DebugReading, DebugSourceList } from './DebugSourceList';
import { useDebugBridge } from './useDebugBridge';
import { debugPhase, debugQuestion, emptyDebug, requestDiff, currentContextSnapshot } from './requestDebug';
import type { DebugSnapshot } from './requestDebug';
import './requestDebug.css';
export function RequestDebugSettings() {
  const revision=useRef(0); const toggling=useRef(false);
  const [snapshot,setSnapshot]=useState<DebugSnapshot>(emptyDebug);
  const [session,setSession]=useState(''); const [selected,setSelected]=useState(0);
  const [tab,setTab]=useState<'context'|'payload'|'diff'|'sources'>('context');
  const [mode,setMode]=useState<'reading'|'json'>('reading');
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
  const bridge=useDebugBridge(record?.sequence);
  const size=(bytes:number)=>bytes<1024?bytes+' B':(bytes/1024).toFixed(1)+' KB';
  return <section className="request-debug" aria-label="请求调试">
    <div className="debug-page-heading"><h2 className="settings-section-title">请求调试</h2><label className="debug-switch"><input type="checkbox" checked={snapshot.enabled} disabled={busy} onChange={()=>{void toggle();}}/>开启请求调试</label></div>
    {error&&<p role="alert" className="inline-error">{error}</p>}
    {!snapshot.enabled?<p role="status" className="debug-notice">已关闭，不采集请求内容。开启后，返回已有对话发送问题，再到这里查看。</p>:!record?<p role="status" className="debug-notice">尚无实际请求。预览或发送前校验失败不会生成记录。</p>:<div className="debug-workspace">
      <aside className="debug-request-rail" aria-label="请求列表"><div className="debug-rail-heading"><h3>实际请求</h3><span>{records.length}</span></div><label className="debug-session-label">会话<select aria-label="调试会话" value={activeSession} onChange={e=>{setSession(e.target.value);setSelected(0);}}>{sessions.map(id=><option key={id}>{id}</option>)}</select></label>
        <div className="debug-request-list">{records.map(r=><button key={r.sequence} className="debug-request-item" aria-label={'请求 #'+r.sequence} aria-pressed={record.sequence===r.sequence} onClick={()=>setSelected(r.sequence)}><span className="debug-request-topline"><strong>#{r.sequence}</strong><time>{r.time?.includes('T')?r.time.split('T')[1].slice(0,5):r.time}</time></span><span className="debug-request-question">{debugQuestion(r)||debugPhase(r)}</span><span className="debug-request-bottomline"><span>{debugPhase(r)}</span><span>{size(r.bodyBytes)}</span></span></button>)}</div>
        <p className="debug-rail-footnote">仅本机临时记录<br/>已淘汰或超限未保留 {snapshot.dropped} 条</p>
      </aside>
      <div className="debug-request-view" key={record.sequence}>
        <header className="debug-request-heading"><div><p>请求 #{record.sequence} · {debugPhase(record)}</p><h3>{debugQuestion(record)||'实际请求 #'+record.sequence}</h3><span>{record.identity.model||record.protocol}</span></div><button className="primary-button debug-export-button" disabled={!bridge.value||bridge.busy} onClick={()=>{void bridge.save();}}><Download size={15}/>{bridge.busy?'正在导出…':'导出解码 JSON'}</button></header>
        <div className="debug-request-facts"><span>{record.status}</span><span>{size(record.bodyBytes)}</span><span>{record.complete?'认证已脱敏':'不完整：'+record.omitted}</span><details className="debug-request-identity"><summary>请求详情 <ChevronRight className="debug-chevron" size={13}/></summary><dl className="debug-identity"><div><dt>time</dt><dd>{record.time}</dd></div><div><dt>runOutcome</dt><dd>{record.runOutcome||'进行中'}</dd></div>{Object.entries(record.identity).map(([key,value])=><div key={key}><dt>{key}</dt><dd>{String(value??'未提供')}</dd></div>)}</dl></details></div>
        {bridge.value&&!bridge.value.complete&&<p className="debug-notice">桥快照不完整，或存在未归属片段。导出会保留完整性说明，不补造缺失内容。</p>}
        {bridge.error&&<p role="alert" className="inline-error">{bridge.error}</p>}
        {bridge.saved&&<p role="status" className="debug-export-result">{bridge.saved}</p>}
        <div className="debug-tabs" role="group" aria-label="请求视图">{([{id:'context',label:'本轮新增上下文'},{id:'sources',label:'完整上下文'},{id:'payload',label:'原始请求'},{id:'diff',label:'与上次差异'}] as const).map(item=><button key={item.id} aria-pressed={tab===item.id} onClick={()=>setTab(item.id)}>{item.label}</button>)}</div>
        {tab==='context'?<><div className="debug-section-toolbar"><div><h4>本轮新增上下文</h4><p>只列本轮实际新注册的桥部件。工具续跑沿用同一用户轮；历史、问题与工具消息见完整上下文。</p></div><div className="debug-mode-switch" role="group" aria-label="上下文显示方式"><button aria-pressed={mode==='reading'} onClick={()=>setMode('reading')}>阅读</button><button aria-pressed={mode==='json'} onClick={()=>setMode('json')}>JSON</button></div></div>{mode==='reading'?<DebugReading record={record}/>:<DebugJsonView value={{complete:record.complete,omitted:record.omitted,entries:currentContextSnapshot(record)}} label="本轮新增上下文JSON"/>}</>:
          tab==='sources'?<><div className="debug-section-toolbar"><div><h4>完整上下文</h4><p>按实际请求顺序保留系统内容、历史消息、压缩摘要及工具调用与结果；未定位来源的消息也完整展示。</p></div><div className="debug-mode-switch" role="group" aria-label="完整上下文显示方式"><button aria-pressed={mode==='reading'} onClick={()=>setMode('reading')}>阅读</button><button aria-pressed={mode==='json'} onClick={()=>setMode('json')}>JSON</button></div></div>{mode==='reading'?<DebugSourceList record={record}/>:<DebugJsonView value={{complete:record.complete,omitted:record.omitted,body:record.body}} label="完整上下文JSON"/>}</>:
          tab==='payload'?<><div className="debug-section-toolbar"><div><h4>原始请求</h4><p>保留线上 JSON 转义；认证字段已脱敏。</p></div></div><DebugJsonView value={{url:record.url,method:record.method,headers:record.headers,body:record.body}} label="原始请求JSON"/></>:
          <><div className="debug-section-toolbar"><div><h4>与上次请求的差异</h4><p>同一会话 · #{record.previousSequence||'无'} → #{record.sequence}</p></div></div><pre tabIndex={0} aria-label="请求差异">{requestDiff(previous,record)}</pre></>}
      </div>
    </div>}
    <p className="debug-footnote">默认关闭。仅保留本机易失内存中的最近 40 条 / 4 MiB，单条请求体上限 256 KiB。开启后按会话连续记录全部请求与工具轮，切换视图或会话不清空；关闭或退出应用才清空，超限自动淘汰最早记录。仅显式导出写入当前 state/exports，不自动上传；已导出文件由你自行管理。</p>
  </section>;
}
