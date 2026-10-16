import { FileText } from 'lucide-react';
import type { RequestedContext } from './types';
export function MessageReferences({ value, onOpen }: { value?: RequestedContext; onOpen: (id: string) => void }) {
 if (!value) return <p className="message-reference-missing">此条历史问题未记录附加引用。</p>;
 const count = value.injections.length + (value.selection ? 1 : 0);
 if (!count) return null;
 return <details className="message-references"><summary><FileText size={12}/>此问附加引用 · {count}</summary><p className="message-reference-note">发送时冻结的请求引用，非已核实模型使用。实际来源见该次回复的来源清单。</p>{value.selection && <div className="message-reference"><strong>本页选句</strong><blockquote>{value.selection}</blockquote>{value.documentId && <button className="text-reference" onClick={() => onOpen(value.documentId)}>打开来源文档</button>}</div>}{value.injections.map(item => <div className="message-reference" key={item.documentId}><strong>{item.selection ? '选段引用' : '全文材料'}</strong>{item.selection && <blockquote>{item.selection}</blockquote>}<small>来源 {item.documentId} · 请求版本 {item.expectedVersion.slice(0,10)}</small><button className="text-reference" onClick={() => onOpen(item.documentId)}>打开来源文档</button></div>)}</details>;
}
