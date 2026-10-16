import { ChevronDown, FileText, LoaderCircle } from 'lucide-react';
import type { RequestContext } from './types';
export type ContextViewState = { status: 'idle' | 'loading' | 'ready' | 'error'; sessionId?: string; context?: RequestContext; actual?: boolean; error?: string };
export function RequestContextView({ value, independent, onRetry }: { value: ContextViewState; independent: boolean; onRetry: () => void }) {
  if (value.status === 'loading') return <div className="context-loading" role="status"><LoaderCircle size={12} className="spin" />正在核对上下文来源…</div>;
  if (value.status === 'error') return <div className="inline-error" role="alert">上下文尚未核对：{value.error}<button onClick={onRetry}>重试核对来源</button></div>;
  if (value.status !== 'ready' || !value.context) return <p className="context-loading">{independent ? '独立探索会话尚未就绪' : '打开小节后核对实际上下文'}</p>;
  const context = value.context;
  return <details className="context-sources"><summary><span className="context-dot" /><span>{value.actual ? '本次实际发送来源' : '下次发送来源（已核对）'} · {context.manifest.length} 项</span><ChevronDown size={13} /></summary><div className="source-details">{context.manifest.length ? context.manifest.map(item => <div className="context-source" key={`${item.id}:${item.version}`}><strong><FileText size={11} />{item.path}</strong><p>已读取 {item.includedChars} 字符 · 版本 {item.version.slice(0, 10)}</p></div>) : <p>{independent ? '未关联当前小节，也不会带入其他小节的历史。' : '本次没有材料来源。'}</p>}{context.warnings.map((warning, index) => <p className="context-warning" key={index}>{warning}</p>)}{context.truncated && <p className="context-warning">上下文达到长度限制，部分内容已截取。</p>}</div></details>;
}
