import { useState } from 'react';
import { ChevronRight, Copy } from 'lucide-react';
import { DebugJsonView } from './DebugJsonView';
import { DebugContextContent } from './DebugContextContent';
import { isCurrentQuestion, currentContextSources, completeContextSources, sourceLabel, sourceValue, sourceRole, sourceRound } from './requestDebug';
import type { DebugRecord, DebugSource } from './requestDebug';
function SourceRow({record,source,index,reading}: {record:DebugRecord;source:DebugSource;index:number;reading?:boolean}) {
  const [copied,setCopied]=useState('');
  const raw=sourceValue(record,source);
  const inlineDescription=typeof raw==='string'&&!!source.description&&raw.startsWith(source.description+'\n');
  const path=source.path.join(' / ')+(source.start!==undefined?' ['+source.start+','+source.end+')':'');
  async function copy() {try{await navigator.clipboard.writeText(path+(source.sourcePath?'\n'+source.sourcePath:''));setCopied('路径已复制');}catch{setCopied('复制失败，可选中路径复制。');}}
  const initialOpen=reading&&(isCurrentQuestion(record,source)||['focus','changes'].includes(source.registrationKey??'')&&sourceRound(record,source)==='本轮'||source.name==='learning-station-material'&&sourceRound(record,source)==='轮次未定位');
  return <details className={reading?'debug-context-card':'debug-source-row'} open={initialOpen?true:undefined} data-source-key={source.registrationKey}>
    <summary><span className="debug-item-number">{String(index+1).padStart(2,'0')}</span><span className="debug-card-title"><span className={'debug-source-badge '+source.origin}>{sourceLabel(source)}</span><strong>{source.registrationKey??source.name}</strong><span className="debug-source-badge">{sourceRole(record,source)} · {sourceRound(record,source)}</span>{source.origin==='bridge'&&!source.registrationKey&&<span className="debug-source-badge unregistered">未注册</span>}<span className="debug-source-path">{path}</span></span><ChevronRight className="debug-chevron" size={14}/></summary>
    <div className="debug-context-body">{source.description&&!inlineDescription&&<p className="debug-source-description">{source.description}</p>}<DebugContextContent record={record} source={source} reading={reading}/><div className="debug-source-location">{source.sourcePath&&<code>{source.sourcePath}</code>}{source.runId&&<span>run · {source.runId}</span>}{source.userEntryId&&<span>userEntry · {source.userEntryId}</span>}<button className="quiet-button" onClick={()=>{void copy();}} aria-label={'复制来源路径 '+String(index+1)}><Copy size={12}/>复制路径</button>{copied&&<span role="status">{copied}</span>}</div></div>
  </details>;
}
export function DebugReading({record}: {record:DebugRecord}) {
  const sources=currentContextSources(record);
  if(!record.complete)return <p className="debug-notice">请求体不完整：{record.omitted}。缺失正文无法展示或导出。</p>;
  if(!sources.length)return <p className="debug-notice">未记录可确认属于本轮的新注册部件。请查看完整上下文；来源或轮次缺失不代表未发送。</p>;
  return <div><p className="debug-notice">仅列有注册键和本轮身份的实发片段；来源未定位的内容请到完整上下文核对。</p>{sources.map((source,index)=><SourceRow key={index} record={record} source={source} index={index} reading/>)}</div>;
}
export function DebugSourceList({record}: {record:DebugRecord}) {
  const sources=completeContextSources(record);
  if(!record.complete)return <p className="debug-notice">请求体不完整：{record.omitted}。无法恢复完整上下文。</p>;
  return <div className="debug-source-layout"><SnapshotDetails record={record}/><div>{sources.length?sources.map((source,index)=><SourceRow key={index} record={record} source={source} index={index}/>):<p className="debug-notice">本请求未记录可识别的 system/messages，请查看原始请求。</p>}
    <details className="debug-tools"><summary>工具定义 · tools <ChevronRight className="debug-chevron" size={14}/></summary><DebugJsonView value={(record.body as {tools?:unknown})?.tools??[]} label="工具定义JSON"/></details></div>
  </div>;
}
function SnapshotDetails({record}:{record:DebugRecord}) {
 const [open,setOpen]=useState(false),[projection,setProjection]=useState(false);
 return <section className="debug-snapshot-card" aria-label="冻结注册详情"><div className="debug-section-toolbar"><div><h4>冻结注册快照 <small>{record.registrations?.length??0} 项</small></h4><p>这是同次冻结的桥输入；最终发送的位置与内容以消息列表为准。</p></div><button className="quiet-button" aria-expanded={open} onClick={()=>setOpen(!open)}>{open?'收起冻结详情':'展开冻结详情'}<ChevronRight className="debug-chevron" size={14}/></button></div>
 {open&&<div className="debug-snapshot-content">{record.registrations?.length?<DebugJsonView value={record.registrations} label="内部注册快照"/>:<p>未记录完整注册快照，不从实发内容反推补造。</p>}{record.snapshotOmitted&&<p className="debug-notice">{record.snapshotOmitted}</p>}{record.projectionRegistrations&&<><button className="quiet-button" aria-expanded={projection} onClick={()=>setProjection(!projection)}>{projection?'收起投影注册值':'查看投影注册值'}<ChevronRight className="debug-chevron" size={14}/></button>{projection&&<><p>Pi 投影的已注册值，包含仍可见的历史片段；不表示本轮全部重新发送。</p><DebugJsonView value={record.projectionRegistrations} label="投影注册快照"/></>}</>}</div>}</section>;
}
