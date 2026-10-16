import { invoke } from '@tauri-apps/api/core';
import type { Bootstrap, Course, Exercise, LearningSession, ModelSettings, ReviewCard, Section, Source, UiDraft } from './types';
import { StationError } from './types';
import { sessionMessages } from './sessionMessages';

type NativeDoc = { id: string; path: string; metadata: { id: string; title: string; kind: string; goals: string[]; tags: string[]; links: string[]; version: number }; body: string; content: string; version: string; modifiedAt: string };
type Summary = { id: string; path: string; title: string; kind: string; tags: string[]; version: string };
type Entry = { id: string; kind: string; content: string; createdAt: string; selection?: string; relatedIds: string[]; data?: Record<string, unknown> };
type Manifest = { id: string; path: string; version: string; includedChars: number };
type NativeSession = { id: string; sectionId: string; title: string; version: string; entries: Entry[]; contextManifest: Manifest[] };
type NativeModel = { configured: boolean; provider: string; baseUrl: string; model: string; verified?: boolean; harness?: string };
type NativeState = { root: string; documents: Summary[]; activeDocumentId?: string; model: NativeModel; lastRunId?: string };
type NativeTodo = { id: string; title: string; dueAt?: string; reminderEnabled?: boolean; done: boolean };
type NativeNotice = { id: string; title: string; dueAt?: string; read: boolean; deliveryStatus?: string };
type Run = { id: string; status: string; content: string; error?: { code: string; message: string } | string; sessionId: string };
const draftVersions = new Map<string, string>();
const draftReadKeys = new Set<string>();
const appendRequests = new Map<string, string>();
const draftQueues = new Map<string, Promise<UiDraft>>();
const summaryTitles = new Map<string, string>();
const documents = new Map<string, NativeDoc>();
const sessions = new Map<string, NativeSession>();
const operationRuns = new Map<string, string>();
const cancelled = new Set<string>();
export type GenerationProgress = { operationId: string; sectionId: string; content: string };
const progressListeners = new Set<(progress: GenerationProgress) => void>();
export function onGenerationProgress(listener: (progress: GenerationProgress) => void) { progressListeners.add(listener); return () => { progressListeners.delete(listener); }; }
const nativeCommands = new Set(['get_app_state', 'refresh_library', 'create_section', 'import_markdown', 'open_document', 'save_document', 'start_session', 'get_session', 'append_entry', 'load_context', 'start_generation', 'cancel_generation', 'get_run', 'configure_model', 'get_model_settings', 'test_model', 'create_todo', 'list_todos', 'set_todo_done', 'list_notifications', 'mark_notification_read', 'export_anki_csv', 'list_review_items', 'get_ui_draft', 'save_ui_draft', 'set_todo_reminder', 'record_ui_metric']);
async function ipc<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (!nativeCommands.has(command)) throw new StationError('不支持的本机命令。', 'INVALID_COMMAND');
  return invoke<T>(command, args);
}
function model(value: NativeModel): ModelSettings { return { ...value, connected: !!value.verified, credentialStored: value.configured }; }
function section(doc: NativeDoc): Section { documents.set(doc.id, doc); return { id: doc.id, title: doc.metadata.title, courseId: 'library', sourcePath: doc.path, body: doc.body, version: doc.version, updatedAt: doc.modifiedAt }; }
function courses(items: Summary[]): Course[] { items.forEach(item => summaryTitles.set(item.id, item.title)); return items.length ? [{ id: 'library', title: '我的英语课程', sections: items.filter(item => item.kind === 'section' || item.kind === 'knowledge').map(item => ({ id: item.id, title: item.title })) }] : []; }
function sources(items: Manifest[]): Source[] { return items.map(item => ({ title: documents.get(item.id)?.metadata.title || summaryTitles.get(item.id) || item.path.split('/').pop() || '关联材料', path: item.path })); }
function learning(value: NativeSession): LearningSession {
  sessions.set(value.sectionId, value);
  const exercises: Exercise[] = [];
  for (const entry of value.entries.filter(entry => entry.kind === 'exercise')) {
    const list = entry.data?.exercises;
    if (!Array.isArray(list)) continue;
    for (const raw of list) {
      const exercise = raw as { id: string; question: string };
      const attempt = value.entries.filter(item => item.kind === 'attempt' && item.data?.exerciseId === exercise.id).at(-1);
      const feedback = value.entries.filter(item => item.kind === 'feedback' && item.data?.exerciseId === exercise.id).at(-1);
      exercises.push({ id: exercise.id, entryId: entry.id, prompt: exercise.question, answer: attempt?.content, feedback: feedback?.content, correct: typeof feedback?.data?.correct === 'boolean' ? feedback.data.correct : undefined });
    }
  }
  const toCard = (entry: Entry): ReviewCard => ({ id: entry.id, sessionId: value.id, sectionId: value.sectionId, front: String(entry.data?.front ?? entry.content), back: String(entry.data?.back ?? ''), source: documents.get(value.sectionId)?.metadata.title || '学习记录' });
  return { id: value.id, sectionId: value.sectionId, sources: sources(value.contextManifest),
    messages: sessionMessages(value.entries, sources(value.contextManifest)),
    exercises,
    mistakes: value.entries.filter(entry => entry.kind === 'error').map(entry => ({ id: entry.id, question: String(entry.data?.front ?? entry.content), reason: String(entry.data?.cause ?? ''), sectionId: value.sectionId })),
    cards: value.entries.filter(entry => entry.kind === 'error' || entry.data?.reviewCard === true).map(toCard),
  };
}
async function sessionFor(sectionId: string) { let value = sessions.get(sectionId); if (!value) { const opened = await ipc<{ session: NativeSession }>('start_session', { sectionId }); value = opened.session; sessions.set(sectionId, value); } return value; }
async function append(sectionId: string, kind: string, content: string, data?: Record<string, unknown>, relatedIds: string[] = [], selection?: string) {
  const key = JSON.stringify({ sectionId, kind, content, data, relatedIds, selection });
  const requestId = appendRequests.get(key) || crypto.randomUUID();
  appendRequests.set(key, requestId);
  const value = await sessionFor(sectionId);
  const args = { sessionId: value.id, entry: { id: '', kind, content, createdAt: '', relatedIds, data, selection }, expectedVersion: value.version, requestId };
  try {
    const updated = await ipc<NativeSession>('append_entry', args);
    sessions.set(sectionId, updated); return updated;
  } catch (error) {
    const code = typeof error === 'object' && error && 'code' in error ? String(error.code) : '';
    const fresh = await ipc<NativeSession>('get_session', { id: value.id }).catch(() => undefined);
    if (fresh) sessions.set(sectionId, fresh);
    if (fresh && ['CONFLICT', 'TIMEOUT', 'NETWORK', 'IO'].includes(code)) {
      const updated = await ipc<NativeSession>('append_entry', { ...args, expectedVersion: fresh.version });
      sessions.set(sectionId, updated); return updated;
    }
    throw error;
  }
}

async function generation(payload: Record<string, unknown>, mode: string, prompt: string) {
  const sectionId = String(payload.sectionId); const operationId = String(payload.operationId); const value = await sessionFor(sectionId);
  let runId: string | undefined;
  try {
    if (payload.draftBody) throw new StationError('请先保存当前小节，再发起生成。问题与选句仍保留。', 'UNSAVED_CONTEXT');
    const started = await ipc<{ runId: string }>('start_generation', { sessionId: value.id, mode, selection: String(payload.selection || ''), prompt, requestId: operationId });
    runId = started.runId;
    operationRuns.set(operationId, runId);
    if (cancelled.has(operationId)) { await ipc('cancel_generation', { runId }); throw new StationError('已请求停止生成，已有记录仍保留。', 'CANCELLED'); }
    const deadline = Date.now() + 130_000;
    while (Date.now() < deadline) {
      if (cancelled.has(operationId)) throw new StationError('已请求停止生成，已有记录仍保留。', 'CANCELLED');
      const run = await ipc<Run>('get_run', { runId });
      if (run.content) progressListeners.forEach(listener => listener({ operationId, sectionId, content: run.content }));
      if (run.status === 'succeeded') return learning(await ipc<NativeSession>('get_session', { id: value.id }));
      if (['failed', 'cancelled', 'interrupted'].includes(run.status)) throw new StationError(typeof run.error === 'string' ? run.error : run.error?.message || '生成未完成，已收到的内容仍保留。', typeof run.error === 'object' ? run.error.code : run.status.toUpperCase());
      await new Promise(resolve => setTimeout(resolve, 350));
    }
    await ipc('cancel_generation', { runId });
    throw new StationError('模型长时间未完成，已请求停止。问题仍保留。', 'TIMEOUT');
  } finally {
    // Starting a run may already have committed a user entry, even if IPC then fails.
    const latest = await ipc<NativeSession>('get_session', { id: value.id }).catch(() => undefined);
    if (latest) sessions.set(sectionId, latest);
    operationRuns.delete(operationId); cancelled.delete(operationId);
  }
}

export async function nativeCall(command: string, payload: Record<string, unknown>): Promise<unknown> {
  switch (command) {
    case 'metric': return ipc('record_ui_metric', { name: payload.name, valueMs: payload.valueMs });
    case 'draft_get': { draftReadKeys.delete(String(payload.key)); const result = await ipc<UiDraft | null>('get_ui_draft', { key: payload.key }); draftReadKeys.add(String(payload.key)); if (result) draftVersions.set(result.key, result.version); else draftVersions.delete(String(payload.key)); return result; }
    case 'draft_save': { const key = String(payload.key); if (!draftReadKeys.has(key)) throw new StationError('请先成功读取原草稿，当前输入仍保留，未覆盖已有草稿。', 'DRAFT_NOT_LOADED'); const previous = draftQueues.get(key); const next = (previous ? previous.catch(() => undefined) : Promise.resolve()).then(async () => { const result = await ipc<UiDraft>('save_ui_draft', { key, data: payload.data, expectedVersion: draftVersions.get(key) }); draftVersions.set(key, result.version); return result; }); draftQueues.set(key, next); return next; }
    case 'task_reminder': { const task = await ipc<NativeTodo>('set_todo_reminder', { id: payload.id, reminderEnabled: payload.reminderEnabled }); return { ...task, completed: task.done }; }
    case 'bootstrap': {
      const state = await ipc<NativeState>('get_app_state');
      const lastRun = state.lastRunId ? await ipc<Run>('get_run', { runId: state.lastRunId }).catch(() => undefined) : undefined;
      const [todos, notices] = await Promise.all([ipc<NativeTodo[]>('list_todos'), ipc<NativeNotice[]>('list_notifications')]);
      return { courses: courses(state.documents), lastSectionId: state.activeDocumentId, model: model(state.model), dataPath: state.root, recovery: lastRun && lastRun.status !== 'succeeded' ? { status: lastRun.status, content: lastRun.content, message: typeof lastRun.error === 'string' ? lastRun.error : lastRun.error?.message } : undefined, tasks: todos.map(item => ({ ...item, completed: item.done })), notices: notices.map(item => ({ ...item, body: item.deliveryStatus === 'disabled' ? '站内提醒 · 飞书未启用' : undefined })) } satisfies Bootstrap;
    }
    case 'library_refresh': return courses(await ipc<Summary[]>('refresh_library'));
    case 'section_open': return section(await ipc<NativeDoc>('open_document', { id: payload.id }));
    case 'section_import': {
      const result = section(await ipc<NativeDoc>(String(payload.body).trimStart().startsWith('```json') ? 'import_markdown' : 'create_section', String(payload.body).trimStart().startsWith('```json') ? { content: payload.body } : { title: payload.title, body: payload.body, goals: [], tags: [] }));
      try { const key = `section:${result.id}`; const draft = await ipc<UiDraft | null>('get_ui_draft', { key }); draftReadKeys.add(key); if (draft) draftVersions.set(key, draft.version); result.draftReady = true; }
      catch { result.draftReady = false; result.draftError = '未能读取原草稿，已暂停草稿备份以保护已有输入。'; }
      return result;
    }
    case 'section_save': {
      const doc = documents.get(String(payload.id)); if (!doc) throw new StationError('请先重新打开小节。', 'NOT_FOUND');
      const content = '```json\n' + JSON.stringify(doc.metadata, null, 2) + '\n```\n\n' + String(payload.body);
      return section(await ipc<NativeDoc>('save_document', { id: payload.id, content, expectedVersion: payload.expectedVersion }));
    }
    case 'session_open': { const result = await ipc<{ session: NativeSession }>('start_session', { sectionId: payload.sectionId }); return learning(result.session); }
    case 'session_refresh': { const value = await sessionFor(String(payload.sectionId)); return learning(await ipc<NativeSession>('get_session', { id: value.id })); }
    case 'question_save': return learning(await append(String(payload.sectionId), 'user', String(payload.question), { mode: 'local', noAi: true }, [], String(payload.selection || '')));
    case 'session_ask': return generation(payload, payload.mode === 'explore' ? 'question' : 'explain', String(payload.question));
    case 'exercise_generate': return generation(payload, 'practice', '请围绕当前小节和选句生成一道针对理解盲点的英语练习。');
    case 'operation_cancel': { const id = String(payload.operationId); const runId = operationRuns.get(id); cancelled.add(id); if (runId) return ipc('cancel_generation', { runId }); return { status: 'cancelling' }; }
    case 'exercise_create_manual': { const exercise = { id: 'manual_' + String(payload.id), question: String(payload.prompt), format: payload.exact ? 'exact' : 'self', answers: [String(payload.reference)], explanation: String(payload.explanation || ''), tags: [], sourceIds: [String(payload.sectionId)] }; return learning(await append(String(payload.sectionId), 'exercise', exercise.question, { exercises: [exercise], manual: true })); }
    case 'exercise_answer': return learning(await append(String(payload.sectionId), 'attempt', String(payload.answer), { exerciseId: payload.exerciseId, exerciseEntryId: payload.entryId, answer: payload.answer }));
    case 'mistake_save': { const value = await sessionFor(String(payload.sectionId)); const previous = value.entries.filter(entry => entry.kind === 'error' && entry.data?.exerciseId === payload.exerciseId).at(-1); if (previous?.data?.cause === payload.reason) return learning(value); return learning(await append(String(payload.sectionId), 'error', String(payload.reason), { exerciseId: payload.exerciseId, front: payload.question, back: payload.reason, cause: payload.reason, tags: [] }, previous ? [previous.id] : [])); }
    case 'card_create': { const value = await append(String(payload.sectionId), 'note', `${payload.front}\n\n${payload.back || ''}`, { front: payload.front, back: payload.back, tags: [], reviewCard: true }); const result = learning(value); return result.cards.at(-1); }
    case 'review_list': return ipc('list_review_items', { query: String(payload.query || '') });
    case 'cards_export': { const sectionId = String(payload.sectionId || ''); const value = sectionId ? await sessionFor(sectionId) : [...sessions.values()].at(-1); if (!value) throw new StationError('请先打开保存过复习卡的小节。', 'NO_SESSION'); return ipc('export_anki_csv', { sessionId: value.id }); }
    case 'model_get': return model(await ipc<NativeModel>('get_model_settings'));
    case 'model_save': return model(await ipc<NativeModel>('configure_model', { provider: payload.provider, baseUrl: payload.baseUrl, model: payload.model, apiKey: String(payload.apiKey || '') }));
    case 'model_test': return ipc('test_model');
    case 'notice_read': return ipc('mark_notification_read', { id: payload.id });
    case 'task_create': { const task = await ipc<NativeTodo>('create_todo', { title: payload.title, dueAt: payload.dueAt, reminderEnabled: payload.reminderEnabled === true, requestId: crypto.randomUUID(), sessionId: sessions.get(String(payload.sectionId))?.id }); return { ...task, completed: task.done }; }
    case 'task_update': { const task = await ipc<NativeTodo>('set_todo_done', { id: payload.id, done: payload.completed }); return { ...task, completed: task.done }; }
    default: throw new StationError('不支持的操作。', 'INVALID_COMMAND');
  }
}
