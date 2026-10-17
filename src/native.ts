import { invoke } from '@tauri-apps/api/core';
import type { Bootstrap, Course, Exercise, LearningSession, ModelSettings, ReviewCard, Section, Source, UiDraft, RequestContext, LibraryNode, LibraryTreeSnapshot, GradeResult, Rubric } from './types';
import { StationError } from './types';
import { sessionMessages } from './sessionMessages';
import { readContextBridgeManifest } from './contextBridge';
import { historicalTools, foldToolActivities } from './toolActivities';
import { noticeBody } from './noticePolicy';
import { freezeRequestedContext } from './requestedContext';
import type { AssistantReference, ContextInjection, ExerciseReference } from './types';
import { stableRunId } from './outgoingMessages';
import type { ToolActivity } from './types';

type NativeDoc = { id: string; path: string; metadata: { id: string; title: string; kind: string; goals: string[]; tags: string[]; links: string[]; version: number; parentId?: string | null; originSessionId?: string; exerciseEntryId?: string; [key: string]: unknown }; body: string; content: string; version: string; modifiedAt: string };
type Summary = { id: string; path: string; title: string; kind: string; tags: string[]; version: string };
type Entry = { id: string; kind: string; content: string; createdAt: string; selection?: string; relatedIds: string[]; data?: Record<string, unknown> };
type Manifest = { id: string; path: string; version: string; includedChars: number };
type NativeSession = { id: string; sectionId: string; title: string; version: string; entries: Entry[]; contextManifest: Manifest[]; contextHistoryManifest?: Manifest[]; contextAssistantReferenceManifest?: Manifest[]; contextExerciseReferenceManifest?: Manifest[]; contextWarnings?: string[]; contextTruncated?: boolean };
type NativeModel = { maxOutputTokens?: number; contextWindow?: number | null; configured: boolean; provider: string; baseUrl: string; model: string; verified?: boolean; harness?: string; protocol?: string; requestUrl?: string; usageGate?: string };
type NativeState = { root: string; documents: Summary[]; activeDocumentId?: string; model: NativeModel; lastRunId?: string };
type NativeTodo = { id: string; title: string; dueAt?: string; reminderEnabled?: boolean; done: boolean };
type NativeNotice = { id: string; title: string; dueAt?: string; read: boolean; deliveryStatus?: string };
type Run = { contextBridgeManifest?: unknown; id: string; status: string; content: string; error?: { code: string; message: string; details?: Record<string, unknown> } | string; sessionId: string; userEntryId?: string; toolCalls?: ToolActivity[]; contextSnapshotVersion?: number; contextManifest?: Manifest[]; historyManifest?: Manifest[]; assistantReferenceManifest?: Manifest[]; exerciseReferenceManifest?: Manifest[]; contextWarnings?: string[]; contextTruncated?: boolean };
const draftVersions = new Map<string, string>();
const draftReadKeys = new Set<string>();
const appendRequests = new Map<string, string>();
const draftQueues = new Map<string, Promise<UiDraft>>();
const summaryTitles = new Map<string, string>();
let libraryNodes: LibraryNode[] = [];
const exerciseSessions = new Map<string, NativeSession>();
const documents = new Map<string, NativeDoc>();
const sessions = new Map<string, NativeSession>();
const sessionIds = new Map<string, NativeSession>();
function cacheSession(value: NativeSession) { sessions.set(value.sectionId, value); sessionIds.set(value.id, value); return value; }
const operationRuns = new Map<string, string>();
const cancelled = new Set<string>();
export type GenerationProgress = { operationId: string; sectionId: string; sessionId: string; content: string; userEntryId?: string; userEntryConfirmed?: boolean; mode?: string; runId?: string; toolCalls?: ToolActivity[]; manifest?: RequestContext['manifest']; context?: RequestContext; contextError?: string };
const progressListeners = new Set<(progress: GenerationProgress) => void>();
export function onGenerationProgress(listener: (progress: GenerationProgress) => void) { progressListeners.add(listener); return () => { progressListeners.delete(listener); }; }
const nativeCommands = new Set(['get_exercise_reference', 'get_app_state', 'refresh_library', 'create_section', 'import_markdown', 'open_document', 'save_document', 'start_session', 'get_session', 'append_entry', 'load_context', 'start_generation', 'cancel_generation', 'get_run', 'configure_model', 'get_model_settings', 'set_model', 'set_model_limits', 'test_model', 'create_todo', 'list_todos', 'set_todo_done', 'list_notifications', 'mark_notification_read', 'export_anki_csv', 'list_review_items', 'get_ui_draft', 'save_ui_draft', 'set_todo_reminder', 'record_ui_metric', 'get_ui_preferences', 'save_ui_preferences', 'start_exploration', 'load_session_context', 'check_feishu_status', 'test_feishu_notification', 'get_library_tree', 'create_library_node', 'move_library_node', 'get_exercise_document', 'create_manual_exercise', 'materialize_legacy_exercise', 'import_course_asset', 'list_course_assets', 'read_course_asset', 'set_review_state', 'export_review_items', 'set_document_links', 'create_exploration_note', 'preview_library_trash', 'trash_library_node', 'list_library_trash', 'restore_library_node', 'list_sessions', 'get_grading_settings', 'set_grading_model', 'grade_attempt', 'get_grade_run', 'cancel_grade_run', 'check_anki_connection', 'get_anki_due', 'reveal_anki_card', 'answer_anki_card', 'get_anki_operation', 'add_review_items_to_anki', 'get_feishu_bot_settings', 'configure_feishu_bot', 'start_feishu_bot', 'stop_feishu_bot', 'get_feishu_bot_snapshot']);
async function ipc<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  if (!nativeCommands.has(command)) throw new StationError('不支持的本机命令。', 'INVALID_COMMAND');
  return invoke<T>(command, args);
}
function model(value: NativeModel): ModelSettings { return { ...value, connected: !!value.verified, credentialStored: value.configured }; }
function section(doc: NativeDoc): Section { documents.set(doc.id, doc); return { id: doc.id, title: doc.metadata.title, courseId: libraryNodes.find(node => node.id === doc.id)?.courseId || doc.id, kind: doc.metadata.kind as Section['kind'], parentId: doc.metadata.parentId, originSessionId: doc.metadata.originSessionId, exerciseEntryId: doc.metadata.exerciseEntryId, links: doc.metadata.links, sourcePath: doc.path, body: doc.body, version: doc.version, updatedAt: typeof doc.metadata.updatedAt === 'string' ? doc.metadata.updatedAt : doc.modifiedAt, createdAt: typeof doc.metadata.createdAt === 'string' ? doc.metadata.createdAt : null, timestampSource: typeof doc.metadata.timestampSource === 'string' ? doc.metadata.timestampSource : undefined }; }
function courses(items: Summary[]): Course[] { items.forEach(item => summaryTitles.set(item.id, item.title)); return items.length ? [{ id: 'library', title: '我的学习材料', sections: items.filter(item => item.kind === 'section' || item.kind === 'knowledge').map(item => ({ id: item.id, title: item.title })) }] : []; }
function sources(items: Manifest[]): Source[] { return items.map(item => ({ title: documents.get(item.id)?.metadata.title || summaryTitles.get(item.id) || item.path.split('/').pop() || '关联材料', path: item.path })); }
function learning(value: NativeSession): LearningSession {
  cacheSession(value);
  const exercises: Exercise[] = [];
  for (const entry of value.entries.filter(entry => entry.kind === 'exercise')) {
    const list = entry.data?.exercises;
    if (!Array.isArray(list)) continue;
    for (const raw of list) {
      const exercise = raw as { id: string; question: string };
      const attempt = value.entries.filter(item => item.kind === 'attempt' && item.data?.exerciseId === exercise.id).at(-1);
      const feedback = attempt && value.entries.filter(item => item.kind === 'feedback' && item.data?.exerciseId === exercise.id && item.data?.attemptId === attempt.id).at(-1);
      const causeEntry = value.entries.filter(item => item.kind === 'error' && item.data?.exerciseId === exercise.id && typeof item.data?.cause === 'string' && item.data.cause).at(-1);
      const cause = causeEntry?.data?.cause;
      exercises.push({ id: exercise.id, entryId: entry.id, prompt: exercise.question, answer: attempt?.content, feedback: feedback?.content, correct: typeof feedback?.data?.correct === 'boolean' ? feedback.data.correct : undefined, format: (raw as { format?: 'exact' | 'self' }).format === 'exact' ? 'exact' : 'self', reference: ((raw as { answers?: string[] }).answers || []).join(' / '), explanation: String((raw as { explanation?: string }).explanation || ''), attemptId: attempt?.id, feedbackEntryId: feedback?.id, rubric: (raw as { rubric?: Rubric }).rubric, aiGrade: feedback?.data?.method === 'ai' && (feedback.data.aiGrade as GradeResult | undefined)?.attemptId === attempt?.id ? feedback.data.aiGrade as GradeResult : undefined, cause: typeof cause === 'string' ? cause : '', causeUpdatedAt: causeEntry?.createdAt });
    }
  }
  const toCard = (entry: Entry): ReviewCard => ({ id: entry.id, sessionId: value.id, sectionId: value.sectionId, front: String(entry.data?.front ?? entry.content), back: String(entry.data?.back ?? ''), source: documents.get(value.sectionId)?.metadata.title || '学习记录' });
  return { id: value.id, sectionId: value.sectionId, sources: sources(value.contextManifest), contextAssistantReferenceManifest: value.contextAssistantReferenceManifest, contextExerciseReferenceManifest: value.contextExerciseReferenceManifest,
    messages: sessionMessages(value.entries), toolCalls: historicalTools(value.entries, value.id),
    exercises,
    mistakes: value.entries.filter(entry => entry.kind === 'error').map(entry => ({ id: entry.id, question: String(entry.data?.front ?? entry.content), reason: String(entry.data?.cause ?? ''), sectionId: value.sectionId })),
    cards: value.entries.filter(entry => entry.kind === 'error' || entry.data?.reviewCard === true).map(toCard),
  };
}
async function sessionFor(sectionId: string, sessionId?: string) {
  if (sessionId) { const cached = sessionIds.get(sessionId); return cached || cacheSession(await ipc<NativeSession>('get_session', { id: sessionId })); }
  const exerciseOrigin = exerciseSessions.get(sectionId); if (exerciseOrigin) return sessionIds.get(exerciseOrigin.id) || exerciseOrigin;
  const cached = sessions.get(sectionId); if (cached) return cached;
  if (!sectionId) throw new StationError('探索会话尚未就绪，请重试打开独立探索。', 'NO_SESSION');
  const opened = await ipc<{ session: NativeSession }>('start_session', { sectionId }); return cacheSession(opened.session);
}
async function append(sectionId: string, kind: string, content: string, data?: Record<string, unknown>, relatedIds: string[] = [], selection?: string, sessionId?: string, requestIdOverride?: string) {
  const key = JSON.stringify({ sectionId, sessionId, kind, content, data, relatedIds, selection });
  const requestId = requestIdOverride || appendRequests.get(key) || crypto.randomUUID();
  appendRequests.set(key, requestId);
  const value = await sessionFor(sectionId, sessionId);
  const args = { sessionId: value.id, entry: { id: '', kind, content, createdAt: '', relatedIds, data, selection }, expectedVersion: value.version, requestId };
  try {
    const updated = await ipc<NativeSession>('append_entry', args);
    cacheSession(updated); return updated;
  } catch (error) {
    const code = typeof error === 'object' && error && 'code' in error ? String(error.code) : '';
    const fresh = await ipc<NativeSession>('get_session', { id: value.id }).catch(() => undefined);
    if (fresh) cacheSession(fresh);
    if (fresh && ['CONFLICT', 'TIMEOUT', 'NETWORK', 'IO'].includes(code)) {
      const updated = await ipc<NativeSession>('append_entry', { ...args, expectedVersion: fresh.version });
      cacheSession(updated); return updated;
    }
    throw error;
  }
}

async function generation(payload: Record<string, unknown>, mode: string, prompt: string) {
  const operationId = String(payload.operationId); const value = await sessionFor(String(payload.sectionId || ''), payload.sessionId ? String(payload.sessionId) : undefined); const sectionId = value.sectionId;
  const runId = await stableRunId(operationId);
  operationRuns.set(operationId, runId);
  let confirmedStatus: string | undefined;
  try {
    if (payload.draftBody) throw new StationError('请先保存当前小节，再发起生成。', 'UNSAVED_CONTEXT');
    let resumed: Run | undefined;
    if (payload.resume === true) {
      try { resumed = await ipc<Run>('get_run', { runId }); }
      catch (error) { if (!(error && typeof error === 'object' && 'code' in error && error.code === 'NOT_FOUND')) throw new StationError('尚不能核实上次生成状态，请稍后再次核对，未新发请求。', 'STATUS_UNKNOWN', { sendStage: 'unknown', runId }); }
    }
    const recoverSavedOutput = resumed?.status === 'failed' && typeof resumed.error === 'object' && resumed.error?.details?.recovery === 'same-operation';
    if (!resumed || recoverSavedOutput) {
      resumed = undefined;
      if (cancelled.has(operationId)) throw new StationError('已停止，问题保存在学习记录中。', 'CANCELLED', { sendStage: 'generation', runId });
      await ipc('start_generation', { sessionId: value.id, mode, selection: String(payload.selection || ''), prompt, requestId: operationId, userEntryId: payload.userEntryId || undefined, injections: payload.injections || [], ...(Array.isArray(payload.assistantReferences) && payload.assistantReferences.length ? { assistantReferences: payload.assistantReferences } : {}), ...(Array.isArray(payload.exerciseReferences) && payload.exerciseReferences.length ? { exerciseReferences: payload.exerciseReferences } : {}) });
    }
    let userSnapshot = await ipc<NativeSession>('get_session', { id: value.id }).then(cacheSession).catch(() => undefined);
    const checkedUserEntries = new Set<string>();
    if (cancelled.has(operationId)) await ipc('cancel_generation', { runId });
    const deadline = Date.now() + 130_000;
    while (Date.now() < deadline) {
      const run = resumed || await ipc<Run>('get_run', { runId }); resumed = undefined;
      confirmedStatus = run.status;
      const matchesUser = () => run.sessionId === value.id && userSnapshot?.id === value.id && typeof run.userEntryId === 'string' && userSnapshot.entries.some(entry => entry.id === run.userEntryId && entry.kind === 'user' && entry.content === prompt && entry.data?.mode === mode && (entry.data?.runId === run.id || payload.userEntryId === run.userEntryId && (entry.selection || '') === String(payload.selection || '')));
      if (mode === 'practice' && run.userEntryId && !matchesUser() && (!checkedUserEntries.has(run.userEntryId) || ['succeeded','failed','cancelled','interrupted'].includes(run.status))) {
        checkedUserEntries.add(run.userEntryId);
        userSnapshot = await ipc<NativeSession>('get_session', { id: value.id }).then(cacheSession).catch(() => undefined);
      }
      const userEntryConfirmed = mode === 'practice' && matchesUser();
      progressListeners.forEach(listener => listener({ operationId, mode, sectionId, sessionId: value.id, runId, userEntryId: run.userEntryId, userEntryConfirmed, content: run.content || '', ...(run.contextSnapshotVersion === 1 && Array.isArray(run.contextManifest) ? { context: { text: '', contextBridgeManifest: readContextBridgeManifest(run.contextBridgeManifest), manifest: run.contextManifest, historyManifest: run.historyManifest || [], assistantReferenceManifest: run.assistantReferenceManifest, exerciseReferenceManifest: run.exerciseReferenceManifest, warnings: run.contextWarnings || [], truncated: run.contextTruncated === true } } : { contextError: '此历史运行未记录实际来源快照，不能用当前会话或空清单代替核实。' }), toolCalls: foldToolActivities(run.toolCalls || [], value.id) }));
      if (run.status === 'succeeded') return learning(await ipc<NativeSession>('get_session', { id: value.id }));
      if (['failed', 'cancelled', 'interrupted'].includes(run.status)) throw new StationError(typeof run.error === 'string' ? run.error : run.error?.message || '生成未完成，已收到的内容仍保留。', typeof run.error === 'object' ? run.error.code : run.status.toUpperCase(), { ...(typeof run.error === 'object' ? run.error.details : {}), sendStage: 'generation', runId, runStatus: run.status });
      await new Promise(resolve => setTimeout(resolve, 350));
    }
    await ipc('cancel_generation', { runId });
    throw new StationError('模型长时间未完成，已请求停止；请先核对状态，避免重复生成。', 'TIMEOUT', { sendStage: 'unknown', runId });
  } catch (error) {
    const data = error && typeof error === 'object' ? error as { code?: string; message?: string; details?: Record<string, unknown> } : {};
    if (data.details?.sendStage) throw error;
    try { confirmedStatus = (await ipc<Run>('get_run', { runId })).status; }
    catch (checkError) { if (checkError && typeof checkError === 'object' && 'code' in checkError && checkError.code === 'NOT_FOUND') confirmedStatus = 'not-started'; }
    const terminal = ['failed', 'cancelled', 'interrupted', 'not-started'].includes(confirmedStatus || '');
    throw new StationError(data.message || '未能读取生成结果，请先核对上次状态。', data.code || 'STATUS_UNKNOWN', { ...data.details, sendStage: terminal ? 'generation' : 'unknown', runId, runStatus: confirmedStatus });
  } finally {
    const latest = await ipc<NativeSession>('get_session', { id: value.id }).catch(() => undefined);
    if (latest) cacheSession(latest);
    operationRuns.delete(operationId); cancelled.delete(operationId);
  }
}

export async function nativeCall(command: string, payload: Record<string, unknown>): Promise<unknown> {
  switch (command) {
    case 'get_exercise_reference': case 'get_feishu_bot_settings': case 'configure_feishu_bot': case 'start_feishu_bot': case 'stop_feishu_bot': case 'get_feishu_bot_snapshot': case 'get_grading_settings': case 'set_grading_model': case 'grade_attempt': case 'get_grade_run': case 'cancel_grade_run': case 'check_anki_connection': case 'get_anki_due': case 'reveal_anki_card': case 'answer_anki_card': case 'get_anki_operation': case 'add_review_items_to_anki': return ipc(command, payload);
    case 'run_get': return ipc('get_run', { runId: payload.id });
    case 'preferences_get': return ipc('get_ui_preferences');
    case 'preferences_save': return ipc('save_ui_preferences', { ...(payload.fontSize === undefined ? {} : { fontSize: payload.fontSize }), ...(payload.assistantWidth === undefined ? {} : { assistantWidth: payload.assistantWidth }), ...(payload.sidebarWidth === undefined ? {} : { sidebarWidth: payload.sidebarWidth }), ...(payload.sidebarCollapsed === undefined ? {} : { sidebarCollapsed: payload.sidebarCollapsed }), ...(payload.expandedNodeIds === undefined ? {} : { expandedNodeIds: payload.expandedNodeIds }) });
    case 'exploration_open': { const result = await ipc<{ session: NativeSession; context: unknown; lastRunId?: string }>('start_exploration', { sectionId: payload.sectionId || undefined, associateCurrentSection: payload.associateCurrentSection === true, fresh: false }); return { session: learning(result.session), context: result.context, lastRunId: result.lastRunId }; }
    case 'context_load': { const session = await sessionFor(String(payload.sectionId || ''), payload.sessionId ? String(payload.sessionId) : undefined); return ipc('load_session_context', { sessionId: session.id, selection: String(payload.selection || ''), injections: payload.injections || [], ...(Array.isArray(payload.assistantReferences) && payload.assistantReferences.length ? { assistantReferences: payload.assistantReferences } : {}), ...(Array.isArray(payload.exerciseReferences) && payload.exerciseReferences.length ? { exerciseReferences: payload.exerciseReferences } : {}) }); }
    case 'activity_refresh': { const [tasks, notices] = await Promise.all([ipc<NativeTodo[]>('list_todos'), ipc<NativeNotice[]>('list_notifications')]); return { tasks: tasks.map(item => ({ ...item, completed: item.done })), notices: notices.map(item => ({ ...item, body: noticeBody(item.deliveryStatus) })) }; }
    case 'metric': return ipc('record_ui_metric', { name: payload.name, valueMs: payload.valueMs });
    case 'draft_get': { draftReadKeys.delete(String(payload.key)); const result = await ipc<UiDraft | null>('get_ui_draft', { key: payload.key }); draftReadKeys.add(String(payload.key)); if (result) draftVersions.set(result.key, result.version); else draftVersions.delete(String(payload.key)); return result; }
    case 'draft_save': { const key = String(payload.key); if (!draftReadKeys.has(key)) throw new StationError('请先成功读取原草稿，当前输入仍保留，未覆盖已有草稿。', 'DRAFT_NOT_LOADED'); const previous = draftQueues.get(key); const next = (previous ? previous.catch(() => undefined) : Promise.resolve()).then(async () => { const result = await ipc<UiDraft>('save_ui_draft', { key, data: payload.data, expectedVersion: draftVersions.get(key) }); draftVersions.set(key, result.version); return result; }); draftQueues.set(key, next); return next; }
    case 'task_reminder': { const task = await ipc<NativeTodo>('set_todo_reminder', { id: payload.id, reminderEnabled: payload.reminderEnabled }); return { ...task, completed: task.done }; }
    case 'bootstrap': {
      const state = await ipc<NativeState>('get_app_state');
      const lastRun = state.lastRunId ? await ipc<Run>('get_run', { runId: state.lastRunId }).catch(() => undefined) : undefined;
      const [todos, notices] = await Promise.all([ipc<NativeTodo[]>('list_todos'), ipc<NativeNotice[]>('list_notifications')]);
      return { courses: courses(state.documents), lastSectionId: state.activeDocumentId, model: model(state.model), dataPath: state.root, recovery: lastRun && lastRun.status !== 'succeeded' ? { status: lastRun.status, content: lastRun.content, message: typeof lastRun.error === 'string' ? lastRun.error : lastRun.error?.message } : undefined, tasks: todos.map(item => ({ ...item, completed: item.done })), notices: notices.map(item => ({ ...item, body: noticeBody(item.deliveryStatus) })) } satisfies Bootstrap;
    }
    case 'library_refresh': return courses(await ipc<Summary[]>('refresh_library'));
    case 'library_tree': { const tree = await ipc<LibraryTreeSnapshot>('get_library_tree'); libraryNodes = tree.nodes; return tree; }
    case 'exploration_note_create': { const result = await ipc<{ document: NativeDoc; session: NativeSession; userEntryId: string; folderId: string }>('create_exploration_note', payload); return { ...result, document: section(result.document), session: learning(result.session) }; }
    case 'history_list': return ipc('list_sessions');
    case 'trash_preview': return ipc('preview_library_trash', { id: payload.id });
    case 'trash_create': return ipc('trash_library_node', payload);
    case 'trash_list': return ipc('list_library_trash');
    case 'trash_restore': return ipc('restore_library_node', payload);
    case 'node_create': return section(await ipc<NativeDoc>('create_library_node', payload));
    case 'document_links': return section(await ipc<NativeDoc>('set_document_links', payload));
    case 'node_move': return section(await ipc<NativeDoc>('move_library_node', payload));
    case 'legacy_exercise_open': return section(await ipc<NativeDoc>('materialize_legacy_exercise', payload));
    case 'asset_list': return ipc('list_course_assets', { courseId: payload.courseId });
    case 'asset_import': return ipc('import_course_asset', payload);
    case 'open_content_url': return ipc('open_content_url', {url:payload.url});
    case 'asset_read': return ipc('read_course_asset', { courseId: payload.courseId, assetId: payload.assetId });
    case 'section_open': { const doc = await ipc<NativeDoc>('open_document', { id: payload.id }); if (doc.metadata.kind === 'exercise') { const result = await ipc<{ session: NativeSession }>('get_exercise_document', { id: payload.id }); exerciseSessions.set(doc.id, cacheSession(result.session)); } return section(doc); }
    case 'section_import': {
      const structured = String(payload.body).trimStart().startsWith('```json');
      const result = section(await ipc<NativeDoc>(structured ? 'import_markdown' : 'create_library_node', structured ? { content: payload.body, parentId: payload.parentId || undefined, requestId: payload.requestId || crypto.randomUUID() } : { kind: 'section', title: payload.title, body: payload.body, parentId: payload.parentId || undefined, requestId: payload.requestId || crypto.randomUUID() }));
      try { const key = `section:${result.id}`; const draft = await ipc<UiDraft | null>('get_ui_draft', { key }); draftReadKeys.add(key); if (draft) draftVersions.set(key, draft.version); result.draftReady = true; }
      catch { result.draftReady = false; result.draftError = '未能读取原草稿，已暂停草稿备份以保护已有输入。'; }
      return result;
    }
    case 'section_save': {
      const doc = documents.get(String(payload.id)); if (!doc) throw new StationError('请先重新打开小节。', 'NOT_FOUND');
      const content = '```json\n' + JSON.stringify(doc.metadata, null, 2) + '\n```\n\n' + String(payload.body);
      return section(await ipc<NativeDoc>('save_document', { id: payload.id, content, expectedVersion: payload.expectedVersion }));
    }
    case 'session_open': { const origin = exerciseSessions.get(String(payload.sectionId)); if (origin) return learning(sessionIds.get(origin.id) || origin); const result = await ipc<{ session: NativeSession }>('start_session', { sectionId: payload.sectionId }); return learning(result.session); }
    case 'session_refresh': { const value = await sessionFor(String(payload.sectionId || ''), payload.sessionId ? String(payload.sessionId) : undefined); return learning(await ipc<NativeSession>('get_session', { id: value.id })); }
    case 'message_save': {
      const sendId = String(payload.sendId);
      const value = await append(String(payload.sectionId || ''), 'user', String(payload.question), { mode: payload.mode, clientMessageId: sendId, requestedContext: freezeRequestedContext(String(payload.sectionId || ''), String(payload.selection || ''), (payload.injections || []) as ContextInjection[], (payload.assistantReferences || []) as AssistantReference[], (payload.exerciseReferences || []) as ExerciseReference[]) }, [], String(payload.selection || ''), String(payload.sessionId), sendId);
      const entry = value.entries.find(item => item.kind === 'user' && item.data?.clientMessageId === sendId);
      if (!entry) throw new StationError('尚未读到这条问题的保存回执，请重试核对。', 'STATUS_UNKNOWN');
      return { session: learning(value), userEntryId: entry.id };
    }
    case 'feishu_status': return ipc('check_feishu_status');
    case 'feishu_test': return ipc('test_feishu_notification', { requestId: payload.requestId });
    case 'tool_object_open': {
      const kind = String(payload.kind); const id = String(payload.id);
      if (kind === 'exercise') return section(await ipc<NativeDoc>('open_document', { id }));
      if (kind === 'todo') {
        const task = (await ipc<NativeTodo[]>('list_todos')).find(item => item.id === id);
        if (!task) throw new StationError('对应待办未找到，请刷新学习待办。', 'NOT_FOUND');
        return { kind, id, title: task.title, dueAt: task.dueAt, done: task.done, reminderEnabled: task.reminderEnabled === true };
      }
      const value = cacheSession(await ipc<NativeSession>('get_session', { id: payload.sessionId }));
      const entry = value.entries.find(item => item.id === id && (item.kind === 'note' || item.kind === 'error'));
      if (!entry || kind === 'review-card' && entry.kind !== 'error' && entry.data?.reviewCard !== true) throw new StationError('对应学习记录未找到，请刷新后重试。', 'NOT_FOUND');
      return { kind, id, title: kind === 'review-card' ? '已保存的复习卡' : '已保存的学习记录', body: entry.content, front: String(entry.data?.front || ''), back: String(entry.data?.back || '') };
    }
    case 'question_save': return learning(await append(String(payload.sectionId), 'user', String(payload.question), { mode: 'local', noAi: true }, [], String(payload.selection || ''), payload.sessionId ? String(payload.sessionId) : undefined));
    case 'session_ask': return generation(payload, payload.mode === 'explore' ? 'question' : 'explain', String(payload.question));
    case 'exercise_generate': return generation(payload, 'practice', String(payload.prompt || '').trim() || '根据当前材料和学习疑点生成针对练习');
    case 'operation_cancel': { const id = String(payload.operationId); const runId = operationRuns.get(id); cancelled.add(id); if (runId) return ipc('cancel_generation', { runId }); return { status: 'cancelling' }; }
    case 'exercise_create_manual': { const origin = await sessionFor(String(payload.sectionId), payload.sessionId ? String(payload.sessionId) : undefined); const result = await ipc<{ document: NativeDoc; session: NativeSession }>('create_manual_exercise', { sessionId: origin.id, sourceDocumentId: payload.sectionId, input: { question: payload.prompt, format: payload.exact ? 'exact' : 'self', answers: [String(payload.reference)], explanation: String(payload.explanation || '') }, requestId: payload.id }); exerciseSessions.set(result.document.id, cacheSession(result.session)); return { session: learning(result.session), document: section(result.document) }; }
    case 'legacy_manual_entry': { const exercise = { id: 'manual_' + String(payload.id), question: String(payload.prompt), format: payload.exact ? 'exact' : 'self', answers: [String(payload.reference)], explanation: String(payload.explanation || ''), tags: [], sourceIds: [String(payload.sectionId)] }; return learning(await append(String(payload.sectionId), 'exercise', exercise.question, { exercises: [exercise], manual: true })); }
    case 'exercise_answer': return learning(await append(String(payload.sectionId), 'attempt', String(payload.answer), { exerciseId: payload.exerciseId, exerciseEntryId: payload.entryId, answer: payload.answer }, [], undefined, payload.sessionId ? String(payload.sessionId) : undefined));
    case 'mistake_save': {
      const value = await sessionFor(String(payload.sectionId), payload.sessionId ? String(payload.sessionId) : undefined);
      const previous = value.entries.filter(entry => entry.kind === 'error' && entry.data?.exerciseId === payload.exerciseId).at(-1);
      if (previous?.data?.cause === payload.reason) return learning(value);
      const question = value.entries.filter(entry => entry.kind === 'exercise').flatMap(entry => Array.isArray(entry.data?.exercises) ? entry.data.exercises : []).find(raw => raw && typeof raw === 'object' && (raw as { id?: string }).id === payload.exerciseId) as { question?: string; answers?: string[]; explanation?: string } | undefined;
      const reference = String(previous?.data?.referenceBack ?? previous?.data?.back ?? `${question?.answers?.join(' / ') || ''}\n${question?.explanation || ''}`.trim());
      const reason = String(payload.reason);
      return learning(await append(String(payload.sectionId), 'error', reason, { exerciseId: payload.exerciseId, front: question?.question || payload.question, back: `${reference}\n\n我的错因：${reason}`, referenceBack: reference, cause: reason, causeConfirmed: true, tags: [] }, previous ? [previous.id] : [], undefined, value.id));
    }
    case 'card_create': { const value = await append(String(payload.sectionId), 'note', `${payload.front}\n\n${payload.back || ''}`, { front: payload.front, back: payload.back, tags: [], reviewCard: true }, [], undefined, payload.sessionId ? String(payload.sessionId) : undefined); const result = learning(value); return result.cards.at(-1); }
    case 'review_list': return ipc('list_review_items', { query: String(payload.query || ''), courseId: payload.courseId || undefined });
    case 'review_state': return ipc('set_review_state', payload);
    case 'review_export': return ipc('export_review_items', { ids: payload.ids });
    case 'legacy_history_open': { const result = await ipc<{ session: NativeSession }>('start_exploration', { associateCurrentSection: false, fresh: false }); return learning(result.session); }
    case 'history_open': return learning(await ipc<NativeSession>('get_session', { id: payload.sessionId }));
    case 'cards_export': { const sectionId = String(payload.sectionId || ''); const value = sectionId ? await sessionFor(sectionId) : [...sessions.values()].at(-1); if (!value) throw new StationError('请先打开保存过复习卡的小节。', 'NO_SESSION'); return ipc('export_anki_csv', { sessionId: value.id }); }
    case 'model_get': return model(await ipc<NativeModel>('get_model_settings'));
    case 'model_save': return model(await ipc<NativeModel>('configure_model', { provider: payload.provider, protocol: payload.protocol, baseUrl: payload.baseUrl, model: payload.model, apiKey: String(payload.apiKey || '') }));
    case 'model_select': return model(await ipc<NativeModel>('set_model', { model: String(payload.model || '') }));
    case 'model_limits_save': return model(await ipc<NativeModel>('set_model_limits', { maxOutputTokens: payload.maxOutputTokens, contextWindow: payload.contextWindow }));
    case 'model_test': return ipc('test_model');
    case 'notice_read': return ipc('mark_notification_read', { id: payload.id });
    case 'task_create': { const task = await ipc<NativeTodo>('create_todo', { title: payload.title, dueAt: payload.dueAt, reminderEnabled: payload.reminderEnabled === true, requestId: crypto.randomUUID(), sessionId: sessions.get(String(payload.sectionId))?.id }); return { ...task, completed: task.done }; }
    case 'task_update': { const task = await ipc<NativeTodo>('set_todo_done', { id: payload.id, done: payload.completed }); return { ...task, completed: task.done }; }
    default: throw new StationError('不支持的操作。', 'INVALID_COMMAND');
  }
}
