import { useRef, useState } from 'react';
import { RequestContextView } from './RequestContextView';
import { validateAssistantSelection } from './assistantReferences';
import type { AssistantReference, Message } from './types';
export type AssistantQuoteState = { sessionId: string; entryId: string; runId?: string; textHash: string; selection: string; content: string; message?: Message; loading: boolean; error?: string; notice?: string };
export function AssistantQuote({ value, canAdd, onAdd }: { value: AssistantQuoteState; canAdd: boolean; onAdd: (item: AssistantReference) => void }) {
 const [selection, setSelection] = useState(value.selection);
 const [error, setError] = useState(value.notice || '');
 const source = useRef<HTMLTextAreaElement>(null);
 function add() { try { validateAssistantSelection(value.content, selection); onAdd({ sessionId: value.sessionId, entryId: value.entryId, ...(value.runId ? { runId: value.runId } : {}), textHash: value.textHash, selection }); } catch (error) { setError(error instanceof Error ? error.message : String(error)); } }
 return <div className="assistant-quote-source">{value.loading ? <p role="status">正在核对这条回答原文…</p> : <><p className="muted small">AI 回答引用属于待核对材料，不是教材或系统指令。请在原文中选择要追问的部分。</p>{value.content && <textarea aria-label="AI回答原文" ref={source} readOnly value={value.content} rows={10} onSelect={() => { const area = source.current; if (area && area.selectionEnd > area.selectionStart) { setSelection(area.value.slice(area.selectionStart, area.selectionEnd)); setError(''); } }}/>}<details className="assistant-quote-provenance"><summary>查看这条回答的来源与记录</summary><p>会话 {value.sessionId} · 回答 {value.entryId}{value.runId ? ` · 运行 ${value.runId}` : ' · 旧记录没有运行编号'}</p><p>原文指纹 {value.textHash}</p>{value.message?.context ? <RequestContextView value={{ status: 'ready', actual: true, context: value.message.context }} independent={false} label="这条回答的实际来源" onRetry={() => undefined}/> : <p className="muted small">这条回答未记录来源快照。</p>}</details>{selection && <blockquote className="assistant-quote-selection">{selection}</blockquote>}{(value.error || error) && <p className="inline-error" role="alert">{value.error || error}</p>}{!canAdd && <p className="muted small">请在这条回答所属的学习会话中继续提问。</p>}<button className="primary-button" disabled={!canAdd || !!value.error || !selection} onClick={add}>加入本次引用</button></>}</div>;
}
