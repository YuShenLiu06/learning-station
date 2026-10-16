import type { Message } from './types';
export type OutgoingMessage = { sendId: string; operationId: string; sessionId: string; sectionId: string; documentId?: string; detached: boolean; mode: string; content: string; selection: string; createdAt: string; userEntryId?: string; stage: 'saving' | 'preparing' | 'generating' | 'failed' | 'done'; failureStage?: 'save' | 'generation' | 'unknown'; error?: string };
export type ConversationMessage = Message & { outgoing?: OutgoingMessage };
export function mergeOutgoing(messages: Message[], outgoing: OutgoingMessage[], sessionId: string): ConversationMessage[] {
  const pending = outgoing.filter(item => item.sessionId === sessionId);
  const matched = new Set<string>();
  const result = messages.map(message => {
    const local = message.role === 'user' ? pending.find(item => item.userEntryId === message.id || item.sendId === message.clientMessageId) : undefined;
    if (local) matched.add(local.sendId);
    return { ...message, outgoing: local };
  });
  for (const local of pending) if (!matched.has(local.sendId)) result.push({ id: local.sendId, clientMessageId: local.sendId, role: 'user', content: local.content, createdAt: local.createdAt, outgoing: local });
  return result;
}
export function reconcileOutgoing(outgoing: OutgoingMessage[], messages: Message[], sessionId: string) {
  return outgoing.map(item => {
    if (item.sessionId !== sessionId || item.userEntryId) return item;
    const saved = messages.find(message => message.role === 'user' && message.clientMessageId === item.sendId);
    return saved ? { ...item, userEntryId: saved.id } : item;
  });
}
export function retryOperation(item: OutgoingMessage, newId: string) {
  return item.failureStage === 'generation' ? newId : item.operationId;
}
export function sendStageLabel(item: OutgoingMessage) {
  if (item.stage === 'saving') return '正在保存问题…';
  if (item.stage === 'preparing') return '问题已保存 · 正在核对来源';
  if (item.stage === 'generating') return '问题已保存 · 正在生成';
  if (item.stage === 'done') return '';
  return item.failureStage === 'unknown' ? '生成结果尚未确认' : item.userEntryId ? '问题已保存 · 生成未完成' : '问题尚未保存';
}
export async function stableRunId(operationId: string) {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(operationId));
  return `run_${Array.from(new Uint8Array(digest), value => value.toString(16).padStart(2, '0')).join('').slice(0, 32)}`;
}
