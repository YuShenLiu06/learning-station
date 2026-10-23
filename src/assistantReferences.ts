import { cloneCitation, validateCitation } from './citation';
import type { AssistantReference } from './types';
import { StationError } from './types';
import { marked } from 'marked';
import { sanitizeMarkdown } from './sanitize';
export async function assistantTextHash(content: string) {
 const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(content));
 return Array.from(new Uint8Array(digest), byte => byte.toString(16).padStart(2, '0')).join('');
}
export function validateAssistantSelection(content: string, selection: string) {
 if (!selection || !content.includes(selection)) throw new StationError('请从这条回答的原文中选择连续内容。', 'CONTEXT_CONFLICT');
 validateCitation(selection);
}
export function addAssistantReference(items: AssistantReference[], item: AssistantReference, documentCount: number) {
 const kept = items.filter(value => value.entryId !== item.entryId);
 if (kept.length >= 4 || kept.length + documentCount >= 8) throw new StationError('最多引用 4 条 AI 回答，与材料合计最多 8 份，请先移除一份。', 'CONTEXT_LIMIT');
 return [...kept, cloneCitation(item)];
}
export const freezeAssistantReferences = (items: AssistantReference[] = []) => items.map(cloneCitation);

// Reuse the application's existing marked + DOMPurify pipeline for presentation and
// conservative rendered-to-source mapping; the request still contains exact raw text.
export function assistantPlainText(content: string) {
 const element = document.createElement('div'); element.innerHTML = sanitizeMarkdown(content);
 return (element.textContent || '').replace(/\s+/gu, ' ').trim();
}
export function assistantReplyContent(content: string) {
 let hidden = false;
 marked.walkTokens(marked.lexer(content), token => { if (token.type === 'html' && /<!--|<\s*(?:script|style|template|think|analysis|reasoning|tool_call|tool_result|iframe|object|embed|head|noscript)\b|\bhidden\b|aria-hidden|data-private/iu.test(token.raw)) hidden = true; });
 if (hidden || !assistantPlainText(content)) throw new StationError('这条回答含不可引用的隐藏内容，或没有可见正文。请选择可见的正文片段回复。', 'OUT_OF_SCOPE');
 validateAssistantSelection(content, content); return content;
}
