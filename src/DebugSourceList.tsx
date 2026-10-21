import { useState } from 'react';
import { ChevronRight, Copy } from 'lucide-react';
import { DebugJsonView } from './DebugJsonView';
import { isCurrentQuestion, orderedSources, sourceContent, sourceLabel, sourceValue, sourceReadingContent, sourceRole, sourceRound } from './requestDebug';
import type { DebugRecord, DebugSource } from './requestDebug';
function SourceRow({record,source,index,reading}: {record:DebugRecord;source:DebugSource;index:number;reading?:boolean}) {
  const [copied,setCopied]=useState('');
  const path=source.path.join(' / ')+(source.start!==undefined?' ['+source.start+','+source.end+')':'');
  async function copy() {try{await navigator.clipboard.writeText(path+(source.sourcePath?'\n'+source.sourcePath:''));setCopied('路径已复制');}catch{setCopied('复制失败，可选中路径复制。');}}
  return <details className={reading?'debug-context-card':'debug-source-row'} open={reading&&(source.name==='learning-station-material'||source.registrationKey==='question')?true:undefined} data-source-key={source.registrationKey}>
    <summary><span className="debug-item-number">{String(index+1).padStart(2,'0')}</span><span className="debug-card-title"><span className={'debug-source-badge '+source.origin}>{sourceLabel(source)}</span><strong>{source.registrationKey??source.name}</strong><span className="debug-source-badge">{sourceRole(record,source)} · {sourceRound(record,source)}</span>{source.origin==='bridge'&&!source.registrationKey&&<span className="debug-source-badge unregistered">未注册</span>}<span className="debug-source-path">{path}</span></span><ChevronRight className="debug-chevron" size={14}/></summary>
    <div className="debug-context-body">{source.description&&<p className="debug-source-description">{source.description}</p>}<pre>{reading?sourceReadingContent(record,source):sourceContent(record,source)}</pre><div className="debug-source-location">{source.sourcePath&&<code>{source.sourcePath}</code>}{source.runId&&<span>run · {source.runId}</span>}{source.userEntryId&&<span>userEntry · {source.userEntryId}</span>}<button className="quiet-button" onClick={()=>{void copy();}} aria-label={'复制来源路径 '+String(index+1)}><Copy size={12}/>复制路径</button>{copied&&<span role="status">{copied}</span>}</div></div>
  </details>;
}
export function DebugReading({record}: {record:DebugRecord}) {
  const sources=orderedSources(record).filter(s=>s.origin==='bridge'||isCurrentQuestion(record,s)||s.origin==='unknown'&&String(sourceValue(record,s)??'').trim());
  if(!record.complete)return <p className="debug-notice">请求体不完整：{record.omitted}。缺失正文无法展示或导出。</p>;
  if(!sources.length)return <p className="debug-notice">本次未记录可定位的上下文正文，请查看原始请求；不能据此推断桥内容为空。</p>;
  return <div>{sources.map((source,index)=><SourceRow key={index} record={record} source={source} index={index} reading/>)}</div>;
}
export function DebugSourceList({record}: {record:DebugRecord}) {
  const sources=orderedSources(record);
  return <div className="debug-source-layout"><div>{sources.length?sources.map((source,index)=><SourceRow key={index} record={record} source={source} index={index}/>):<p className="debug-notice">本请求未记录来源定位，请查看原始请求。</p>}
    <details className="debug-tools"><summary>工具定义 · tools <ChevronRight className="debug-chevron" size={14}/></summary><DebugJsonView value={(record.body as {tools?:unknown})?.tools??[]} label="工具定义JSON"/></details></div>
    <aside className="debug-snapshot-card"><span>冻结输入</span><h4>内部注册快照 <small>{record.registrations?.length??0} 项</small></h4><p>桥的输入，不表示最终实发顺序；同一 key 可以对应多个实发片段。</p><details><summary>查看注册快照 <ChevronRight className="debug-chevron" size={14}/></summary>{record.registrations?.length?<DebugJsonView value={record.registrations} label="内部注册快照"/>:<p>未记录完整注册快照，不从实发内容反推补造。</p>}</details>{record.projectionRegistrations&&<details><summary>投影注册快照 <ChevronRight className="debug-chevron" size={14}/></summary><DebugJsonView value={record.projectionRegistrations} label="投影注册快照"/></details>}</aside>
  </div>;
}
