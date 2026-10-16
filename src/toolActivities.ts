import type { ToolActivity } from './types';
const titles: Record<string, string> = { query_related_knowledge: '查找相关学习记录', record_question: '记录困惑', create_error: '记录错题', create_review_card: '创建复习卡', create_note: '保存学习笔记', create_todo: '创建待办', generate_exercises: '生成练习' };
const safeId = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(value);
export function readToolActivity(raw: unknown): ToolActivity | null {
  if (!raw || typeof raw !== 'object') return null;
  const value = raw as Record<string, unknown>;
  if (!safeId(value.id) || !safeId(value.toolCallId) || !safeId(value.runId) || !safeId(value.sessionId) || typeof value.name !== 'string' || !Object.hasOwn(titles, value.name) || !['running', 'succeeded', 'failed'].includes(String(value.status))) return null;
  const result: ToolActivity = { id: value.id, toolCallId: value.toolCallId, runId: value.runId, sessionId: value.sessionId, name: value.name, status: value.status as ToolActivity['status'], title: titles[value.name], summary: '', startedAt: typeof value.startedAt === 'string' ? value.startedAt : '', updatedAt: typeof value.updatedAt === 'string' ? value.updatedAt : '', persisted: value.persisted === true };
  const target = value.object as Record<string, unknown> | undefined;
  if (target && ['todo', 'learning-entry', 'review-card', 'exercise'].includes(String(target.kind)) && safeId(target.id) && safeId(target.sessionId) && (target.sectionId === '' || safeId(target.sectionId))) result.object = { kind: target.kind as NonNullable<ToolActivity['object']>['kind'], id: target.id, sessionId: target.sessionId, sectionId: target.sectionId };
  if (typeof value.errorCode === 'string' && /^[A-Z_]{1,40}$/u.test(value.errorCode)) result.errorCode = value.errorCode;
  // Only fixed UI summaries are rendered; model arguments, paths and raw output are never copied.
  result.summary = result.status === 'failed' ? '工具未完成，未确认保存成功。' : result.name === 'query_related_knowledge' ? '相关学习记录查询已完成，未写入新内容。' : result.persisted && result.object ? '已收到本机保存回执。' : '等待本机工具结果。';
  return result;
}
export function foldToolActivities(values: unknown[], sessionId: string) {
  const latest = new Map<string, ToolActivity>();
  for (const raw of values) {
    const item = readToolActivity(raw); if (!item || item.sessionId !== sessionId) continue;
    const previous = latest.get(item.id);
    if (previous && (previous.status !== 'running' && item.status === 'running' || previous.updatedAt > item.updatedAt)) continue;
    latest.set(item.id, item);
  }
  return [...latest.values()];
}
export function toolState(item: ToolActivity, liveRunId?: string): 'running' | 'succeeded' | 'failed' | 'unconfirmed' {
  if (item.status === 'running') return item.runId === liveRunId ? 'running' : 'unconfirmed';
  if (item.status === 'succeeded' && item.name !== 'query_related_knowledge' && (!item.persisted || !item.object)) return 'unconfirmed';
  return item.status;
}
export function historicalTools(entries: { kind: string; data?: Record<string, unknown> }[], sessionId: string) {
  return foldToolActivities(entries.filter(entry => entry.kind === 'note' && entry.data?.origin === 'pi-tool-activity').map(entry => entry.data?.toolActivity), sessionId);
}
