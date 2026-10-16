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
 if (Array.from(selection).length > 8000 || new TextEncoder().encode(selection).length > 30000) throw new StationError('每条回答引用最多 8000 字符、30000 字节，请缩小选段。', 'CONTEXT_LIMIT');
}
export function addAssistantReference(items: AssistantReference[], item: AssistantReference, documentCount: number) {
 const kept = items.filter(value => value.entryId !== item.entryId);
 if (kept.length >= 4 || kept.length + documentCount >= 8) throw new StationError('最多引用 4 条 AI 回答，与材料合计最多 8 份，请先移除一份。', 'CONTEXT_LIMIT');
 return [...kept, { ...item }];
}
export const freezeAssistantReferences = (items: AssistantReference[] = []) => items.map(item => ({ ...item }));

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
export function assistantReplySelection(body: HTMLElement, content: string, selected: Selection | null) {
 if (!selected || selected.isCollapsed || !selected.rangeCount) return undefined;
 if (selected.rangeCount !== 1 || !body.contains(selected.anchorNode) || !body.contains(selected.focusNode)) throw new StationError('请只选同一条回答的正文，不能跨消息或选入工具记录。', 'OUT_OF_SCOPE');
 const range = selected.getRangeAt(0), text = range.toString();
 if (!text.trim()) throw new StationError('选区只有空白。请缩小选区，或取消选区后回复整条回答。', 'OUT_OF_SCOPE');
 const parent = (node: Node) => node.nodeType === 1 ? node as Element : node.parentElement;
 const privateSelector = 'button,input,textarea,[hidden],[aria-hidden="true"],[data-private]';
 if (parent(range.startContainer)?.closest(privateSelector) || parent(range.endContainer)?.closest(privateSelector) || range.cloneContents().querySelector(privateSelector)) throw new StationError('只能引用可见回答正文，不能选入隐藏内容或操作控件。', 'OUT_OF_SCOPE');
 if (content.includes(text)) { validateAssistantSelection(content, text); return text; }
 // Locate the rendered range in the raw source, then accept it only when the
 // existing renderer produces exactly the selected visible text. Never guess
 // offsets from a different message or broaden a partial selection to the whole.
 const before = document.createRange(); before.selectNodeContents(body); before.setEnd(range.startContainer, range.startOffset);
 const start = before.toString().length, end = start + text.length, rendered = body.textContent || '';
 const positions: number[] = []; let cursor = 0;
 for (let index = 0; index < rendered.length; index++) {
  if (/\s/u.test(rendered[index])) { positions[index] = -1; continue; }
  const position = content.indexOf(rendered[index], cursor); if (position < 0) throw new StationError('此选段无法精确对应原文，请缩小到一句话或一个短语后回复。', 'CONTEXT_CONFLICT');
  positions[index] = position; cursor = position + 1;
 }
 const covered = positions.slice(start, end).filter(position => position >= 0);
 if (!covered.length) throw new StationError('选区没有可引用的正文。', 'OUT_OF_SCOPE');
 let left = covered[0], right = covered.at(-1)! + 1;
 while (left > 0 && /[*_~`]/u.test(content[left - 1])) left--;
 while (right < content.length && /[*_~`]/u.test(content[right])) right++;
 const candidate = content.slice(left, right);
 if (assistantPlainText(candidate) !== text.replace(/\s+/gu, ' ').trim()) throw new StationError('此选段无法精确对应原文，请缩小到一句话或一个短语后回复。', 'CONTEXT_CONFLICT');
 assistantReplyContent(candidate); return candidate;
}
