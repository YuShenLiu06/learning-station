import { Highlighter, Layers2, MessageCircle, Sparkles, X } from 'lucide-react';
import type { ContextInjection } from './types';

export type ComposerReference = ContextInjection & { title: string; currentSelection?: boolean };
export function composerReferences(selection: string, documentId: string, version: string, title: string, injections: ContextInjection[], titleFor: (id: string) => string): ComposerReference[] {
 const items: ComposerReference[] = injections.map(item => ({ ...item, title: titleFor(item.documentId) }));
 if (selection && !items.some(item => item.documentId === documentId && item.selection === selection)) items.unshift({ documentId, expectedVersion: version, selection, title, currentSelection: true });
 return items;
}

// The existing quote actions stay together above the scrolling conversation.
export function SelectionContextTray({ references, busy, onRemove, onAsk, onPractice, onReview }: {
 references: ComposerReference[]; busy: boolean; onRemove: (item: ComposerReference) => void;
 onAsk: () => void; onPractice: () => void; onReview: (quote: string) => void;
}) {
 if (!references.length) return null;
 const quote = references.find(item => item.selection)?.selection || '';
 return <section className="selection-tray" aria-label="选句与本次上下文"><div className="selection-quotes" aria-label={references.some(item => !item.currentSelection) ? "本次显式上下文" : undefined}>{references.map(item => <div key={`${item.currentSelection ? 'selection' : 'injection'}:${item.documentId}`} {...(!item.currentSelection ? { 'data-explicit-context': true } : {})}><div className="selected-quote"><Highlighter size={13}/><span>{item.currentSelection ? '当前选句' : '本次加入'} · {item.title}</span><button className="icon-button" aria-label={item.currentSelection ? '取消选句' : `移除上下文 ${item.documentId}`} disabled={busy} onClick={() => onRemove(item)}><X size={13}/></button></div>{item.selection ? <blockquote>{item.selection}</blockquote> : <p className="context-reference-whole">全文 · 发送前核对材料版本</p>}</div>)}</div><div className="selection-actions"><button onClick={onAsk}><MessageCircle size={12}/>问一句</button><button disabled={busy} onClick={onPractice}><Sparkles size={12}/>考考我</button><button disabled={!quote} onClick={() => onReview(quote)}><Layers2 size={12}/>留作复习</button></div></section>;
}

export function ComposerReferenceChips({ references, busy, onRemove }: { references: ComposerReference[]; busy: boolean; onRemove: (item: ComposerReference) => void }) {
 if (!references.length) return null;
 return <div className="composer-references" aria-label="本次提问附加引用">{references.map(item => <div className="composer-reference" key={`${item.currentSelection ? 'selection' : 'injection'}:${item.documentId}`}><span title={item.selection || `${item.title} · 全文`}>{item.title}<small>{item.selection ? item.selection : '全文'}</small></span><button className="icon-button" aria-label={item.currentSelection ? '移除提问选句' : `移除提问引用 ${item.documentId}`} disabled={busy} onClick={() => onRemove(item)}><X size={12}/></button></div>)}</div>;
}
