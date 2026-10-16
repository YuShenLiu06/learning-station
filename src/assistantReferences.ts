import type { AssistantReference } from './types';
import { StationError } from './types';
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
