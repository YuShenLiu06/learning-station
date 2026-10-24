import type { Message, ToolActivity } from './types';
const titles: Record<string, string> = { query_related_knowledge: '查找相关学习记录', record_question: '记录困惑', create_error: '记录错题', create_review_card: '创建复习卡', create_note: '保存学习笔记', create_todo: '创建待办', generate_exercises: '生成练习', list_learning_skills: '查看学习指导目录', load_learning_skill: '加载学习指导', read: '受控读取', write: '受控写入', bash: '受控命令' };
const readOnlyTool = (name: string) => name === 'query_related_knowledge' || name === 'list_learning_skills' || name === 'load_learning_skill' || name === 'read';
const safeId = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(value);
// Whitelist mirroring the backend exercise validator's field diagnostics exactly:
// src-tauri/src/exercises.rs field_error builds `练习校验失败：{path} {reason}；收到的内容已保留。`
// (path vocabulary: schema|summary|exercises[N, up to 3 digits because the backend loop can index past 99 before the ≤5 filter] with optional .id|.question|.rubric|.format|.answers[M ≤10, filtered before per-answer validation]|.explanation|.tags|.sourceIds;
// reasons are a fixed set where only the char-count rule interpolates digits, capped by the 256KiB payload check),
// plus the single fixed literal of src-tauri/src/generation.rs practice() (field "json"). Fully anchored so raw
// field values, URLs or arbitrary model/user content can never pass; anything unmatched falls back to fixed copy.
const FIELD_PATHS = '(?:schema|summary|exercises(?:\\[\\d{1,3}\\](?:\\.(?:id|question|rubric|format|explanation|tags|sourceIds|answers(?:\\[\\d{1,2}\\])?))?)?)';
const FIELD_REASONS = '必须是文本|不能为空或只有空白|不能包含空字符|必须是对象且不能包含未声明字段|必须是数组|必须包含1至5道练习|评分标准结构、编号或分值无效|必须是exact或self|必须包含1至10个答案|必须是最多12项的数组|必须是最多32项的数组|来源编号必须是文本|含本轮授权来源清单以外的编号|来源编号重复|共\\d{1,6}个字符，最多(?:64|128|10000|20000)个字符';
const SAFE_FIELD_DIAGNOSTIC = new RegExp(`^(?:练习校验失败：${FIELD_PATHS} (?:${FIELD_REASONS})；收到的内容已保留。|模型没有返回严格练习 JSON；请仅输出exercises与summary组成的JSON对象，不带代码围栏或说明；收到的内容已保留。)$`, 'u');
export function readToolActivity(raw: unknown): ToolActivity | null {
  if (!raw || typeof raw !== 'object') return null;
  const value = raw as Record<string, unknown>;
  if (!safeId(value.id) || !safeId(value.toolCallId) || !safeId(value.runId) || !safeId(value.sessionId) || typeof value.name !== 'string' || !Object.hasOwn(titles, value.name) || !['running', 'waiting', 'unknown', 'succeeded', 'failed'].includes(String(value.status))) return null;
  const result: ToolActivity = { id: value.id, toolCallId: value.toolCallId, runId: value.runId, sessionId: value.sessionId, name: value.name, status: value.status as ToolActivity['status'], title: titles[value.name], summary: '', startedAt: typeof value.startedAt === 'string' ? value.startedAt : '', updatedAt: typeof value.updatedAt === 'string' ? value.updatedAt : '', persisted: value.persisted === true };
  if (safeId(value.userEntryId)) result.userEntryId = value.userEntryId;
  if (Number.isSafeInteger(value.sequence) && (value.sequence as number) >= 0) result.sequence = value.sequence as number;
  const target = value.object as Record<string, unknown> | undefined;
  if (target && ['todo', 'learning-entry', 'review-card', 'exercise'].includes(String(target.kind)) && safeId(target.id) && safeId(target.sessionId) && (target.sectionId === '' || safeId(target.sectionId))) result.object = { kind: target.kind as NonNullable<ToolActivity['object']>['kind'], id: target.id, sessionId: target.sessionId, sectionId: target.sectionId };
  if (typeof value.errorCode === 'string' && /^[A-Z_]{1,40}$/u.test(value.errorCode)) result.errorCode = value.errorCode;
  // Failed cards keep fixed UI copy unless the backend exercise validator supplied a field-level
  // INVALID_MODEL_OUTPUT diagnostic that passes SAFE_FIELD_DIAGNOSTIC (strict whitelist above);
  // overlong or off-vocabulary text falls back. Model arguments and raw output are never rendered.
  const diagnostic = result.status === 'failed' && result.errorCode === 'INVALID_MODEL_OUTPUT' && typeof value.summary === 'string' && value.summary.length <= 80 && SAFE_FIELD_DIAGNOSTIC.test(value.summary) ? value.summary : '';
  result.summary = result.status === 'failed' ? (diagnostic || '工具未完成，未确认保存成功。') : result.name === 'query_related_knowledge' ? '相关学习记录查询已完成，未写入新内容。' : result.name === 'read' ? '受控读取已完成，未写入内容。' : ['list_learning_skills', 'load_learning_skill'].includes(result.name) ? '内置学习指导读取已完成，未写入内容。' : result.persisted && (result.object || ['write', 'bash'].includes(result.name)) ? '已收到本机保存回执。' : '等待本机工具结果。';
  if (result.name === 'write' && result.status === 'failed' && result.errorCode === 'CONFLICT' && [
    '已有修改建议待审阅，本次未写入；请先接受或拒绝旧建议，再重新读取后修改。',
    '旧建议结果尚未核实，本次未写入；请在学习站核对原建议，不要重复写入。',
    '文件版本不匹配，本次未写入；请重新读取并使用 read 返回的 version，不要使用 Focus 的 Git OID。',
  ].includes(String(value.summary))) result.summary = String(value.summary);
  if (result.name === 'write' && !result.persisted && ['AI 修改待确认，建议已保留，正文未修改。', '用户已拒绝建议，正文未修改。', '内容未变化，未重复写入。'].includes(String(value.summary))) result.summary = String(value.summary);
  return result;
}
export function foldToolActivities(values: unknown[], sessionId: string) {
  const latest = new Map<string, ToolActivity>();
  for (const raw of values) {
    const item = readToolActivity(raw); if (!item || item.sessionId !== sessionId) continue;
    const previous = latest.get(item.id);
    if (previous && (previous.status !== 'running' && item.status === 'running' || previous.updatedAt > item.updatedAt)) continue;
    if (previous?.runId === item.runId) {
      item.userEntryId ??= previous.userEntryId;
      item.sequence ??= previous.sequence;
    }
    latest.set(item.id, item);
  }
  return [...latest.values()];
}
export function toolState(item: ToolActivity, liveRunId?: string): 'running' | 'waiting' | 'succeeded' | 'failed' | 'unconfirmed' {
  if (item.name === 'write' && ['用户已拒绝建议，正文未修改。', '内容未变化，未重复写入。'].includes(item.summary)) return 'succeeded';
  if (item.status === 'unknown') return 'unconfirmed';
  if (item.status === 'waiting') return 'waiting';
  if (item.status === 'running') return item.runId === liveRunId ? 'running' : 'unconfirmed';
  if (item.status === 'succeeded' && !readOnlyTool(item.name) && (!item.persisted || (!item.object && !['write', 'bash'].includes(item.name)))) return 'unconfirmed';
  return item.status;
}
export function historicalTools(entries: { kind: string; data?: Record<string, unknown> }[], sessionId: string) {
  return foldToolActivities(entries.filter(entry => entry.kind === 'note' && entry.data?.origin === 'pi-tool-activity').map(entry => entry.data?.toolActivity), sessionId);
}

// Resolve only against stable identities in the visible conversation window.
// A retry keeps its original question; no timestamp/nearest-message inference.
export function toolsByMessage(messages: Message[], tools: ToolActivity[]) {
  const users = new Set(messages.filter(message => message.role === 'user').map(message => message.id));
  const answers = new Map(messages.filter(message => message.role === 'assistant' && message.runId || message.toolRunId).map(message => [message.toolRunId || message.runId!, message.id]));
  const result = new Map<string, ToolActivity[]>();
  for (const tool of tools) {
    const anchor = tool.userEntryId && users.has(tool.userEntryId) ? tool.userEntryId : answers.get(tool.runId);
    if (!anchor) continue;
    const items = result.get(anchor) || [];
    items.push(tool);
    result.set(anchor, items);
  }
  for (const [anchor, items] of result) {
    const runs = new Map<string, ToolActivity[]>();
    for (const item of items) runs.set(item.runId, [...(runs.get(item.runId) || []), item]);
    result.set(anchor, [...runs.values()].flatMap(run => run.sort((a, b) => (a.sequence ?? Number.MAX_SAFE_INTEGER) - (b.sequence ?? Number.MAX_SAFE_INTEGER))));
  }
  return result;
}
