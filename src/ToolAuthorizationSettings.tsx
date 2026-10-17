import { useState } from 'react';
import { call } from './api';

type ToolReceiptSummary = { requestId: string; tool: string; status: string; createdAt: string; path?: string | null };
const RECEIPT_TOOL_LABELS: Record<string, string> = { write: '写入', bash: '命令' };
const RECEIPT_STATUS_LABELS: Record<string, string> = { persisted: '已写入', unverified: '未确认', completed: '已执行', 'timed-out': '已超时' };
function receiptTime(value: string) { const time = new Date(value); return Number.isNaN(time.getTime()) ? value : time.toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }); }

export function ToolAuthorizationSettings({ blocked, sessionId }: { blocked: boolean; sessionId: string }) {
  const [receiptsOpen, setReceiptsOpen] = useState(false);
  const [receipts, setReceipts] = useState<ToolReceiptSummary[] | null>(null);
  const [receiptsTruncated, setReceiptsTruncated] = useState(false);
  const [receiptsError, setReceiptsError] = useState('');
  async function loadReceipts() {
    if (!sessionId) return;
    setReceiptsError(''); setReceipts(null);
    try {
      const value = await call<{ receipts?: unknown; truncated?: boolean }>('tool_receipts', { sessionId });
      const list = Array.isArray(value.receipts) ? value.receipts.filter((item): item is ToolReceiptSummary =>
        !!item && typeof item === 'object' && typeof (item as ToolReceiptSummary).requestId === 'string') : [];
      setReceipts(list); setReceiptsTruncated(value.truncated === true);
    } catch (error) {
      setReceiptsError(error instanceof Error ? error.message : String(error));
    }
  }
  function toggleReceipts() {
    const next = !receiptsOpen;
    setReceiptsOpen(next);
    if (next) { setReceipts(null); setReceiptsError(''); void loadReceipts(); }
  }
  return <section className="tool-authorization-settings" aria-label="工具授权与回执"><h2 className="settings-section-title">工具授权</h2>
    <p className="muted small">档位在输入框左下角随取随选（关闭/受控读取/受控读写/受控命令），默认关闭，不会替你开启。{blocked ? '当前正在生成，不能变更档位。' : '生成进行中不能变更档位。'}此处核对已落账的工具回执。</p>
    <div>
      <button className="quiet-button" type="button" aria-expanded={receiptsOpen} onClick={toggleReceipts} disabled={!sessionId}>{receiptsOpen ? '收起本会话工具回执' : '查看本会话工具回执'}</button>
      {!sessionId && <p className="muted small">先打开一个学习会话，才能查看它的工具回执。</p>}
      {receiptsOpen && sessionId && <>
        {receiptsError && <div className="inline-error" role="alert">{receiptsError}<button onClick={() => { void loadReceipts(); }}>重试读取</button></div>}
        {!receipts && !receiptsError && <p className="muted small">正在读取已落账回执…</p>}
        {receipts && receipts.length === 0 && <p className="muted small">本会话还没有已落账的写入或命令回执。</p>}
        {receipts && receipts.length > 0 && <ul className="task-list" aria-label="本会话工具回执">{receipts.map(item => <li className="task-item" key={item.requestId}><div><span>{RECEIPT_TOOL_LABELS[item.tool] || item.tool} · {RECEIPT_STATUS_LABELS[item.status] || item.status}</span>{item.path && <span className="label-note">{item.path}</span>}</div><small title={item.requestId}>{receiptTime(item.createdAt)} · {item.requestId.slice(0, 16)}</small></li>)}{receiptsTruncated && <li className="task-item"><div><span>回执较多，仅显示最近 200 条。</span></div></li>}</ul>}
        <p className="muted small">只读核对：列表不包含命令原文、参数或文件内容。</p>
      </>}
    </div>
  </section>;
}
