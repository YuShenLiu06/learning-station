import { useEffect, useMemo, useRef } from 'react';
import { CornerUpLeft } from 'lucide-react';
import { assistantPlainText } from './assistantReferences';
import type { Message } from './types';
export type AssistantReplyMenuState = { sessionId: string; message: Message; selection?: string; locator?: import('./types').CitationLocator; x: number; y: number; trigger: HTMLElement };
// Reuses the local context-menu surface. Focus/escape behavior follows Radix's
// documented menu pattern without introducing a second UI dependency stack.
export function AssistantReplyMenu({ value, onReply, onClose }: { value: AssistantReplyMenuState; onReply: () => void; onClose: (restoreFocus?: boolean) => void }) {
 const menu = useRef<HTMLDivElement>(null);
 useEffect(() => { menu.current?.querySelector('button')?.focus(); const outside = (event: PointerEvent) => { if (!menu.current?.contains(event.target as Node)) onClose(false); }; window.addEventListener('pointerdown', outside); return () => window.removeEventListener('pointerdown', outside); }, []);
 return <div ref={menu} className="tree-context-menu assistant-reply-menu" role="menu" aria-label="回答操作" style={{left:Math.max(8,Math.min(value.x,window.innerWidth-190)),top:Math.max(8,Math.min(value.y,window.innerHeight-65))}} onKeyDown={event=>{if(event.key==='Escape'){event.preventDefault();event.stopPropagation();onClose(true);}else if(['ArrowUp','ArrowDown','Home','End'].includes(event.key)){event.preventDefault();menu.current?.querySelector('button')?.focus();}else if(event.key==='Tab'){event.preventDefault();onClose(true);}}}><button type="button" role="menuitem" onClick={onReply}><CornerUpLeft size={15}/>回复</button></div>;
}
export function AssistantReplyPreview({ text, onOpen }: { text: string; onOpen?: () => void }) {
 const preview = useMemo(()=>{const plain=assistantPlainText(text);return Array.from(plain).length>160?Array.from(plain).slice(0,160).join('')+'…':plain;},[text]);
 const content=<><CornerUpLeft size={13}/><span>回复学习助手<small>{preview}</small></span></>;
 return onOpen?<button type="button" className="assistant-reply-preview" aria-label="回到原回答" title="回到原回答" onClick={onOpen}>{content}</button>:<div className="assistant-reply-preview">{content}</div>;
}
