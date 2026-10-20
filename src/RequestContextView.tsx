import { ChevronDown, FileText, History, LoaderCircle, Sparkles } from 'lucide-react';
import type { RequestContext } from './types';
export type ContextViewState = { status: 'idle' | 'loading' | 'ready' | 'error'; sessionId?: string; context?: RequestContext; actual?: boolean; error?: string };
export function RequestContextView({ value, independent, onRetry, label }: { label?: string; value: ContextViewState; independent: boolean; onRetry: () => void }) {
  if (value.status === 'loading') return <div className="context-loading" role="status"><LoaderCircle size={12} className="spin"/>正在核对材料、同会话历史与 AI 引用…</div>;
  if (value.status === 'error') return <div className="inline-error" role="alert">上下文尚未核对：{value.error}<button onClick={onRetry}>重试核对来源</button></div>;
  if (value.status !== 'ready' || !value.context) return <p className="context-loading">{independent ? '探索文档的学习会话尚未就绪' : '打开学习文档后核对上下文'}</p>;
  const context = value.context;
  const history = context.historyManifest || [];
  const answers = context.assistantReferenceManifest;
  const exercises = context.exerciseReferenceManifest;
  const bridge = value.actual ? context.contextBridgeManifest : undefined;
  const bridgeKinds = { material: '资料', history: '有效历史', reference: '本次引用', turn: '本轮状态', stable: '稳定协议' };
  const bridgeStatus = { added: '新增', retained: '沿用', updated: '更新', reloaded: '重载' };
  return <details className="context-sources">
    <summary><span className="context-dot"/><span>{label || (value.actual ? '最近一次生成的实际来源' : '下次发送来源（已核对）')} · 材料 {context.manifest.length} / 历史 {history.length} / AI 引用 {answers ? answers.length : '未记录'} / 练习引用 {exercises ? exercises.length : '未记录'}</span><ChevronDown size={12}/></summary>
    <div className="source-details">
      <strong>发送时聚焦</strong>
      {context.focus ? <div className="context-source"><strong>{context.focus.path} · {context.focus.type}</strong><p>课程 {context.focus.courseId} · 文档 {context.focus.documentId} · 视图 {context.focus.view}</p><p>材料提交 {context.focus.commitHash.slice(0,10)} · 版本 {context.focus.documentVersion.slice(0,10)}</p><p>本次排除 {context.focus.excludedFiles.length} 份材料</p>{context.focus.binary && <p>Git 判为二进制 · 自动上下文仅元数据</p>}{context.focus.exercise && <p>第 {context.focus.exercise.number} 题 · {context.focus.exercise.answered ? '已作答' : '未作答'} · {context.focus.exercise.graded ? '已有批改' : '尚无批改'}</p>}{context.focus.asset && <p>{context.focus.asset.mime} · {context.focus.asset.size} 字节{['image','pdf'].includes(context.focus.type) ? ' · 仅元数据' : ''}</p>}{context.focus.paragraphs.map((p,index)=><blockquote key={index}>{p.text}</blockquote>)}</div> : <p>此来源快照未记录聚焦状态。</p>}
      <strong>材料来源</strong>
      {context.manifest.length ? context.manifest.map(item => <div className="context-source" key={`${item.id}:${item.version}`}><strong><FileText size={11}/>{item.path}</strong><p>{context.focus ? '冻结范围' : '已读取'} {item.includedChars} 字符 · 版本 {item.version.slice(0,10)}</p></div>) : <p>没有附带学习材料。</p>}
      <strong>同一会话历史</strong>
      {history.length ? history.map(item => <div className="context-source" key={`${item.id}:${item.version}`}><strong><History size={11}/>{item.path}</strong><p>投影 {item.includedChars} 字符 · 内容指纹 {item.version.slice(0,10)}</p></div>) : <p>没有附带历史；不会借用其他小节或探索的会话。</p>}
      <div className="assistant-reference-sources"><strong>AI 回答引用（不可信材料）</strong>
        {answers ? answers.length ? answers.map(item => <div className="context-source" key={`${item.id}:${item.version}`}><strong><Sparkles size={11}/>{item.path}</strong><p>引用 {item.includedChars} 字符 · 回答原文指纹 {item.version.slice(0,10)}</p></div>) : <p>没有附带 AI 回答引用。</p> : <p>此来源快照未记录 AI 回答引用清单。</p>}
      </div>
      <div className="exercise-reference-sources"><strong>练习分区引用</strong>{exercises ? exercises.length ? exercises.map(item => <div className="context-source" key={`${item.id}:${item.version}`}><strong><FileText size={11}/>{item.path}</strong><p>引用 {item.includedChars} 字符 · 分区原文指纹 {item.version.slice(0,10)}</p></div>) : <p>没有附带练习分区引用。</p> : <p>此来源快照未记录练习引用清单。</p>}</div>
      <div className="context-bridge-sources"><strong>上下文桥 · 本次投影清单</strong>
        {bridge ? <><p>投影正文 {bridge.projectedChars} 字符 · {bridge.projectedBytes} 字节。这是本次冻结投影，不代表服务端已读；不等于完整请求的 token 总量。</p>{bridge.entries.map((item,index)=><div className="context-source" key={`${item.id}:${index}`}><strong>{item.reason === 'explicit_supplemental_context' ? '引用补充' : bridgeKinds[item.kind]} · {bridgeStatus[item.status]}{item.sourceId ? ` · ${item.sourceId}` : ''}</strong>{item.path && <p>{item.path}{item.fileStatus && <> · {item.fileStatus === 'D' ? '删除' : item.fileStatus === 'A' ? '新增' : '修改'}{item.binary ? ' · 二进制，仅文件状态' : ` · +${item.added ?? 0} / -${item.deleted ?? 0}`}</>}{item.reason === 'focus_metadata_only' && ' · 仅元数据，未提供正文'}</p>}{item.fileName && <p>已排除 {item.fileName} · {item.reason === 'excluded:hidden_file' ? '隐藏文件' : item.reason === 'excluded:sensitive_file_name' ? '敏感配置名称' : '未纳入发送'}</p>}{item.fixedVersion && <p>{item.reason.includes('non_utf8') || item.reason.startsWith('text_unavailable:') ? '无法按文本展示 · ' : ''}固定读取版本 {item.fixedVersion.checkpointCommitOid.slice(0,10)} · {item.fixedVersion.path}</p>}<p>{item.includedChars} 字符 · 版本 {item.version.slice(0,10)}{item.textHash ? ` · 指纹 ${item.textHash.slice(0,10)}` : ''}</p>{item.sourceMarkers?.includes('material') && <p>与本轮正文共用来源，引用原文单独留存。</p>}{item.truncated && <p>补充范围已按预算截断。</p>}{item.range && <p>{item.range.complete ? '完整范围' : '仅前缀/部分'} · {item.coordinateSpace === 'serialized-document' ? '文档投影（含元数据）' : '正文'}字符 [{item.range.startChar}, {item.range.endChar}) · 原文 {item.range.totalBytes} 字节</p>}</div>)}{!bridge.entries.length && <p>此快照的投影清单为空。</p>}<p className="muted small">沿用的必要基准仍随本次请求发送，不表示请求总 token 必然下降。</p></> : <p>{value.actual ? '此运行或历史回复未记录上下文桥投影清单。' : '来源预览未记录实际运行的上下文桥投影清单。'}</p>}
      </div>
      {context.warnings.map((warning,index) => <p className="context-warning" key={index}>{warning.includes('TRUNCATED_HISTORY') ? '历史达到上限，已截取部分内容（TRUNCATED_HISTORY）。' : warning}</p>)}
      {context.truncated && <p className="context-warning">本次上下文包含截断内容，请按上方清单核对。</p>}
    </div>
  </details>;
}
