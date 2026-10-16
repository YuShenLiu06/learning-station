import { Highlighter, Layers2, MessageCircle, Sparkles, X } from 'lucide-react';
import type { AssistantReference, ContextInjection } from './types';

export type ComposerReference = (ContextInjection & { kind?: 'document'; title: string; currentSelection?: boolean }) | (AssistantReference & { kind: 'assistant'; title: string; currentSelection?: false });
const referenceKey = (item: ComposerReference) => item.kind === 'assistant' ? `assistant:${item.entryId}` : `${item.currentSelection ? 'selection' : 'injection'}:${item.documentId}`;
export function composerReferences(selection: string, documentId: string, version: string, title: string, injections: ContextInjection[], titleFor: (id: string) => string, assistantReferences: AssistantReference[] = []): ComposerReference[] {
 const items: (ContextInjection & { title: string; currentSelection?: boolean })[] = injections.map(item => ({ ...item, title: titleFor(item.documentId) }));
 if (selection && !items.some(item => item.documentId === documentId && item.selection === selection)) items.unshift({ documentId, expectedVersion: version, selection, title, currentSelection: true });
 return [...items, ...assistantReferences.map(item => ({ ...item, kind: 'assistant' as const, title: 'AI 回答选段' }))];
}

// The existing quote actions stay together above the scrolling conversation.
export function SelectionContextTray({ references, busy, onRemove, onAsk, onPractice, onReview, onOpen }: {
 references: ComposerReference[]; busy: boolean; onRemove: (item: ComposerReference) => void;
 onAsk: () => void; onPractice: () => void; onReview: (quote: string) => void; onOpen?: (item: AssistantReference) => void;
}) {
 if (!references.length) return null;
 const quote = references.find(item => item.selection)?.selection || '';
 return <section className="selection-tray" aria-label="选句与本次上下文"><div className="selection-quotes" aria-label={references.some(item => !item.currentSelection) ? "本次显式上下文" : undefined}>{references.map(item => <div key={referenceKey(item)} {...(!item.currentSelection ? { 'data-explicit-context': true } : {})}><div className="selected-quote"><Highlighter size={13}/><span>{item.currentSelection ? '当前选句' : '本次加入'} · {item.title}</span><button className="icon-button" aria-label={item.kind === 'assistant' ? `移除回答引用 ${item.entryId}` : item.currentSelection ? '取消选句' : `移除上下文 ${item.documentId}`} onClick={() => onRemove(item)}><X size={13}/></button></div>{item.selection ? <blockquote>{item.selection}</blockquote> : <p className="context-reference-whole">全文 · 发送前核对材料版本</p>}{item.kind === 'assistant' && onOpen && <button className="text-reference" onClick={() => onOpen(item)}>展开回答原文与来源</button>}</div>)}</div><div className="selection-actions"><button onClick={onAsk}><MessageCircle size={12}/>问一句</button><button disabled={busy} onClick={onPractice}><Sparkles size={12}/>考考我</button><button disabled={!quote} onClick={() => onReview(quote)}><Layers2 size={12}/>留作复习</button></div></section>;
}

export function ComposerReferenceChips({ references, onRemove }: { references: ComposerReference[]; busy: boolean; onRemove: (item: ComposerReference) => void }) {
 if (!references.length) return null;
 return <div className="composer-references" aria-label="本次提问附加引用">{references.map(item => <div className="composer-reference" key={referenceKey(item)}><span title={item.selection || `${item.title} · 全文`}>{item.title}<small>{item.selection ? item.selection : '全文'}</small></span><button className="icon-button" aria-label={item.kind === 'assistant' ? `移除提问回答引用 ${item.entryId}` : item.currentSelection ? '移除提问选句' : `移除提问引用 ${item.documentId}`} onClick={() => onRemove(item)}><X size={12}/></button></div>)}</div>;
}
