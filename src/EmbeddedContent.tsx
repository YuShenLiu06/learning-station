import { useEffect, useMemo, useState } from 'react';
import DOMPurify from 'dompurify';
import { call, nativeAvailable } from './api';
import { embedMarkdown, MAX_HTML_CHARS, safeRemoteUrl, type EmbeddedContent } from './embedPolicy';

export function isolatedHtml(content: string): string {
  const body = DOMPurify.sanitize(content, { WHOLE_DOCUMENT: true, FORBID_TAGS:['script','iframe','object','embed','base','meta','link','form'], FORBID_ATTR:['srcset','action','formaction','href','target'], ALLOW_DATA_ATTR:false });
  return `<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'none'; style-src 'unsafe-inline'; img-src data:; media-src 'none'; connect-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"><meta name="referrer" content="no-referrer"></head><body>${body}</body></html>`;
}
export function EmbeddedPreview({ value }: { value: EmbeddedContent }) {
  const [open,setOpen] = useState(false), [attempt,setAttempt] = useState(0), [waiting,setWaiting] = useState(false), [error,setError] = useState('');
  const html = useMemo(()=>value.kind === 'html' ? isolatedHtml(value.content) : undefined,[value]);
  const remote = value.kind !== 'html' ? safeRemoteUrl(value.source) : undefined;
  useEffect(()=>{ if (!open) return; setWaiting(true); const timer=setTimeout(()=>setWaiting(false),8000); return ()=>clearTimeout(timer); },[open,attempt]);
  async function original() {
    if (!remote) return;
    try { if(nativeAvailable) await call('open_content_url',{url:remote}); else window.open(remote,'_blank','noopener,noreferrer'); }
    catch { setError('原链接未能打开，请复制链接到浏览器。'); }
  }
  return <section className="embedded-content" aria-label="隔离内容预览">
    <div className="embed-actions"><strong>{value.kind === 'html' ? value.source : value.kind === 'video' ? '视频' : '网页'}</strong><button type="button" className="quiet-button" onClick={()=>{setOpen(!open);setError('');}}>{open?'收起预览':'打开预览'}</button>{open&&<button type="button" className="quiet-button" onClick={()=>{setAttempt(attempt+1);setError('');}}>重新加载</button>}</div>
    <p className="muted small">{value.kind === 'html' ? '本地 HTML 静态预览：脚本、外部资源、表单与本机接口均禁用；原始内容完整保留。' : '远程内容仅在打开预览后联网；没有本机文件或原生接口权限。站点限制不会被绕过。'}</p>
    {remote&&<><p className="embed-source">{value.source}</p><button type="button" className="quiet-button" onClick={()=>{void original();}}>在浏览器打开原链接</button></>}
    {open && (html || remote) && <>
      <iframe key={attempt} title={value.source} sandbox={value.kind === 'html'?'':'allow-scripts'} srcDoc={html} src={remote} referrerPolicy="no-referrer" allow="camera 'none'; microphone 'none'; geolocation 'none'; clipboard-read 'none'; clipboard-write 'none'; payment 'none'; usb 'none'" onLoad={()=>setWaiting(false)} onError={()=>{setWaiting(false);setError('加载失败，请重新加载或打开原链接。');}}/>
      {waiting&&<p role="status">正在载入…</p>}
      {remote&&<p className="muted small">如显示空白，可能是站点禁止嵌入、离线或链接失效。可重新加载或打开原链接；载入事件不能证明远程内容可用。</p>}
    </>}
    {error&&<p role="alert" className="inline-error">{error}</p>}
    {value.kind === 'html'&&<details><summary>查看原始 HTML（来源文件内容）</summary><pre><code>{value.content}</code></pre></details>}
  </section>;
}
export function InsertContent({ onInsert }: { onInsert: (text:string)=>void }) {
  const [kind,setKind]=useState<'html'|'web'|'video'>('html'),[source,setSource]=useState('演示.html'),[content,setContent]=useState(''),[error,setError]=useState('');
  async function file(selected?:File) { if(!selected)return; if(selected.size>MAX_HTML_CHARS){setError('HTML 文件最多200KB。');return;}try{setContent(await selected.text());setSource(selected.name);setError('');}catch{setError('文件未能读取，请重新选择。');} }
  return <form onSubmit={event=>{event.preventDefault();if(kind==='html'&&(!content.trim()||content.length>MAX_HTML_CHARS)){setError('请填写 HTML，最多200000字符。');return;}if(kind!=='html'&&!safeRemoteUrl(source)){setError('请使用无凭据的公共 HTTPS 原链接。');return;}onInsert(embedMarkdown(kind==='html'?{kind,source,content}:{kind,source}));}}>
    <label>内容类型<select value={kind} onChange={event=>{const next=event.target.value as typeof kind;setKind(next);setSource(next==='html'?'演示.html':'');setError('');}}><option value="html">本地 HTML</option><option value="web">网页</option><option value="video">视频嵌入链接</option></select></label>
    <label>{kind==='html'?'来源文件名':'原链接'}<input aria-label="嵌入来源" value={source} onChange={event=>setSource(event.target.value)} maxLength={kind==='html'?200:2048} required/></label>
    {kind==='html'?<><label>选择 HTML 文件<input type="file" accept=".html,.htm,text/html" onChange={event=>{void file(event.target.files?.[0]);}}/></label><label>或粘贴 HTML<textarea aria-label="HTML 原文" value={content} onChange={event=>setContent(event.target.value)} rows={8}/></label><p className="muted">保存原文到笔记；预览禁用脚本和外部资源。</p></>:<p className="muted">视频请使用网站提供的 HTTPS 嵌入链接。需登录或禁止嵌入的内容可从原链接打开。</p>}
    {error&&<p className="inline-error" role="alert">{error}</p>}<div className="dialog-footer"><button className="primary-button">插入笔记</button></div>
  </form>;
}
