import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react';
import { ArrowDownToLine, ArrowLeft, ArrowRight, ArrowUp, BookOpen, Check, CheckCheck, ChevronDown, ChevronRight, CircleHelp, Clock3, FileText, FolderOpen, Highlighter, Layers2, ListTodo, LoaderCircle, MessageCircle, Maximize2, Minimize2, MoreHorizontal, PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen, PencilLine, Plus, Search, Settings2, Sparkles, Square, X } from 'lucide-react';
import { call, emptyBootstrap, isDemo, nativeAvailable, recordMetric, sampleBody } from './api';
const Editor = lazy(() => import('./Editor').then(module => ({ default: module.Editor })));
import { modelAccessReason, modelAccessMessage, modelBindingChanged, modelFailureMessage, modelFormError, supportedProtocol } from './modelPolicy';
import { assertExplorationScope } from './explorationPolicy';
import { useIndependentExploration } from './useIndependentExploration';
import { RequestContextView } from './RequestContextView';
import type { ContextViewState } from './RequestContextView';
import { useFontSize } from './useFontSize';
import { Markdown } from './Markdown';
import { composerKeyAction } from './keyboard';
import { draftInputsDiffer, questionForSection, unbackedSectionHasInput } from './draftPolicy';
import { flushExerciseDrafts, hasExerciseInput, useExerciseDraft } from './exerciseDraft';
import { onGenerationProgress } from './native';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { GenerationProgress } from './native';
import type { Bootstrap, Course, Exercise, LearningSession, ModelSettings, ReviewCard, Section, Source, Task, UiDraft, RequestContext } from './types';
import { StationError } from './types';

type View = 'lesson' | 'review' | 'tasks';
type Panel = 'explain' | 'explore' | 'practice';
type DocState = { section: Section; body: string; savedBody: string; saving: boolean; error?: string; conflict?: boolean; recovery?: boolean; draftLoaded?: boolean };
type CardDraft = { front: string; back: string };
const emptySession = (id: string): LearningSession => ({ id: '', sectionId: id, messages: [], exercises: [], mistakes: [], cards: [] });
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);
const makeId = () => crypto.randomUUID();

export function App() {
  const font = useFontSize();
  const fontRef = useRef(font); fontRef.current = font;
  const [boot, setBoot] = useState<Bootstrap>(emptyBootstrap);
  const [loading, setLoading] = useState(true);
  const [bootError, setBootError] = useState('');
  const [draftErrors, setDraftErrors] = useState<Record<string, string>>({});
  function reportDraftError(key: string, error: unknown) { setDraftErrors(old => ({ ...old, [key]: errorText(error) })); }
  function clearDraftError(key: string) { setDraftErrors(old => { const next = { ...old }; delete next[key]; return next; }); }
  const [activeId, setActiveId] = useState('');
  const [docs, setDocs] = useState<Record<string, DocState>>({});
  const docsRef = useRef(docs); docsRef.current = docs;
  const [sessions, setSessions] = useState<Record<string, LearningSession>>({});
  const [opening, setOpening] = useState(false);
  const [openError, setOpenError] = useState('');
  const [view, setView] = useState<View>('lesson');
  const editorStartedAt = useRef<Record<string, number>>({});
  function switchView(next: View) { const started = performance.now(); setView(next); requestAnimationFrame(() => recordMetric('page-switch', performance.now() - started)); }
  useEffect(() => { const frame = requestAnimationFrame(() => recordMetric('shell-interactive', performance.now())); return () => cancelAnimationFrame(frame); }, []);
  const [editing, setEditing] = useState(false);
  const [editedIds, setEditedIds] = useState<Set<string>>(new Set());
  const [panel, setPanel] = useState<Panel>('explain');
  const [associateSection, setAssociateSection] = useState(true);
  const independentMode = panel === 'explore' && !associateSection;
  const independent = useIndependentExploration(independentMode);
  const independentRef = useRef(independent); independentRef.current = independent;
  const [linkedOpening, setLinkedOpening] = useState(false);
  const [linkedError, setLinkedError] = useState('');
  const linkedSequence = useRef(0);
  const [contextView, setContextView] = useState<ContextViewState>({ status: 'idle' });
  const [contextRetry, setContextRetry] = useState(0);
  const contextSequence = useRef(0);
  const [sidebar, setSidebar] = useState(true);
  const [rightbar, setRightbar] = useState(true);
  const [immersive, setImmersive] = useState(false);
  const assistantScroll = useRef<HTMLDivElement>(null);
  const assistantScrollPositions = useRef({ normal: 0, immersive: 0 });
  function toggleImmersive() {
    const scroll = assistantScroll.current;
    if (scroll) assistantScrollPositions.current[immersive ? 'immersive' : 'normal'] = scroll.scrollTop;
    setImmersive(!immersive);
    requestAnimationFrame(() => { if (scroll) scroll.scrollTop = assistantScrollPositions.current[immersive ? 'normal' : 'immersive']; });
  }
  const [selection, setSelection] = useState('');
  const [question, setQuestion] = useState('');
  const [localQuestionSaving, setLocalQuestionSaving] = useState(false);
  const [busyId, setBusyId] = useState('');
  const [partial, setPartial] = useState<GenerationProgress | null>(null);
  useEffect(() => onGenerationProgress(progress => { if (progress.operationId !== busyRef.current) return; if (progress.content) setPartial(progress); if (progress.manifest) setContextView({ status: 'ready', actual: true, sessionId: progress.sessionId, context: { text: '', manifest: progress.manifest, warnings: [], truncated: false } }); if (progress.contextError) setContextView({ status: 'error', sessionId: progress.sessionId, error: progress.contextError }); }), []);
  const busyRef = useRef('');
  const [aiError, setAiError] = useState('');
  const [importing, setImporting] = useState(false);
  const materialDraft = useRef({ title: '', body: '' });
  const [settings, setSettings] = useState(false);
  const [manualExercise, setManualExercise] = useState(false);
  const [backupConflict, setBackupConflict] = useState<UiDraft | null>(null);
  const workspaceDraftLoaded = useRef(false);
  const [comparison, setComparison] = useState<Section | null>(null);
  const [cardDraft, setCardDraft] = useState<CardDraft | null>(null);
  const [toast, setToast] = useState('');
  const [filter, setFilter] = useState('');
  const [reviewQuery, setReviewQuery] = useState('');
  const [cards, setCards] = useState<ReviewCard[]>([]);
  const [reviewLoading, setReviewLoading] = useState(false);
  const [reviewError, setReviewError] = useState('');
  useEffect(() => {
    if (!nativeAvailable) return;
    const subscription = listen<{ id: string; title: string; read: boolean }>('notification', event => { setBoot(old => ({ ...old, notices: [...old.notices.filter(item => item.id !== event.payload.id), { ...event.payload, body: '站内提醒 · 飞书未启用' }] })); });
    return () => { void subscription.then(unlisten => unlisten()); };
  }, []);
  const [showBack, setShowBack] = useState<Record<string, boolean>>({});
  const [taskTitle, setTaskTitle] = useState('');
  const [taskDue, setTaskDue] = useState('');
  const [taskReminder, setTaskReminder] = useState(false);
  const [taskBusy, setTaskBusy] = useState(false);
  const [taskError, setTaskError] = useState('');
  const [notifications, setNotifications] = useState(false);
  const questionRef = useRef<HTMLTextAreaElement>(null);
  const composerComposing = useRef(false);
  const composerCommitFrame = useRef(0);
  const questionDrafts = useRef<Record<string, string>>({});
  const activeIdRef = useRef(activeId); activeIdRef.current = activeId;
  const chatEnd = useRef<HTMLDivElement>(null);
  const openSequence = useRef(0);
  const current = docs[activeId];
  const session = independentMode ? independent.session ?? emptySession('') : sessions[activeId] ?? emptySession(activeId);
  const composerQuestion = independentMode ? independent.question : question;
  const requestSelection = independentMode ? '' : selection;
  const canCompose = independentMode ? !!independent.session : !!current;
  const visibleSessionRef = useRef(session.id); visibleSessionRef.current = session.id;
  const modelBlocked = !!modelAccessReason(boot.model);
  const modelReady = boot.model.configured && !modelBlocked;
  const dirtyCount = Object.values(docs).filter(doc => doc.body !== doc.savedBody).length;
  const activeCourse = boot.courses.find(course => course.sections.some(section => section.id === activeId));

  const notify = useCallback((message: string) => setToast(message), []);
  useEffect(() => { if (activeId) questionDrafts.current[activeId] = question; }, [activeId, question]);
  useEffect(() => { if (toast) { const timer = setTimeout(() => setToast(''), 4500); return () => clearTimeout(timer); } }, [toast]);
  const openSection = useCallback(async (id: string) => {
    const started = performance.now();
    const sequence = ++openSequence.current;
    setActiveId(id); setQuestion(questionForSection(questionDrafts.current, id)); switchView('lesson'); setEditing(false); setSelection(''); setAiError(''); setOpenError('');
    if (docsRef.current[id]) {
      if (nativeAvailable) void call<Section>('section_open', { id }).then(section => {
        const local = docsRef.current[id];
        if (!local || local.saving || section.version === local.section.version) return;
        if (local.body !== local.savedBody) { setDocs(old => ({ ...old, [id]: { ...old[id], conflict: true, error: '原文件已在外部更新。当前编辑保留，请先对照新版本。' } })); return; }
        setDocs(old => ({ ...old, [id]: { section, body: section.body, savedBody: section.body, saving: false, draftLoaded: local.draftLoaded } }));
        setEditedIds(old => { const next = new Set(old); next.delete(id); return next; });
      }).catch(error => setOpenError(errorText(error)));
      return;
    }
    setOpening(true);
    try {
      const [section, draft] = await Promise.all([call<Section>('section_open', { id }), nativeAvailable ? call<UiDraft | null>('draft_get', { key: `section:${id}` }).then(value => ({ loaded: true, value })).catch(error => { reportDraftError(`section:${id}`, error); return { loaded: false, value: null }; }) : Promise.resolve({ loaded: true, value: null })]);
      setDocs(previous => ({ ...previous, [id]: { section, body: draft.value?.data.body ?? section.body, savedBody: section.body, saving: false, recovery: draft.value?.data.body !== undefined && draft.value.data.body !== section.body, draftLoaded: draft.loaded } }));
      void call<LearningSession>('session_open', { sectionId: id }).then(learning => setSessions(previous => ({ ...previous, [id]: learning }))).catch(error => { if (activeIdRef.current === id) setAiError(`小节已打开，学习记录暂未载入：${errorText(error)}`); });
      requestAnimationFrame(() => recordMetric('section-ready', performance.now() - started));
      if (draft.value?.data.question) { questionDrafts.current[id] = draft.value.data.question; if (sequence === openSequence.current) setQuestion(draft.value.data.question); }
    } catch (error) { if (sequence === openSequence.current) setOpenError(errorText(error)); }
    finally { if (sequence === openSequence.current) setOpening(false); }
  }, []);
  const load = useCallback(async () => {
    setLoading(true); setBootError('');
    try {
      const result = await call<Bootstrap>('bootstrap');
      setBoot(result);
      const id = result.lastSectionId || result.courses[0]?.sections[0]?.id;
      if (id) void openSection(id);
      if (!isDemo) void call<Course[]>('library_refresh').then(courses => { setBoot(old => ({ ...old, courses })); if (id && !docsRef.current[id]) void openSection(id); else if (!id && courses[0]?.sections[0]) void openSection(courses[0].sections[0].id); }).catch(error => setBootError(errorText(error)));
    } catch (error) { setBootError(errorText(error)); }
    finally { setLoading(false); }
  }, [openSection]);
  useEffect(() => { void load(); if (nativeAvailable) void call<UiDraft | null>('draft_get', { key: 'workspace' }).then(draft => { if (draft && (materialDraft.current.body || materialDraft.current.title) && draftInputsDiffer('workspace', { importTitle: materialDraft.current.title, importBody: materialDraft.current.body }, draft.data)) { workspaceDraftLoaded.current = false; setBackupConflict(draft); reportDraftError('workspace', '发现不同的导入草稿，请先对照选择。'); return; } workspaceDraftLoaded.current = true; if (draft && !materialDraft.current.body && !materialDraft.current.title) materialDraft.current = { title: draft.data.importTitle || '', body: draft.data.importBody || '' }; }).catch(error => reportDraftError('workspace', error)); }, [load]);
  useEffect(() => {
    const beforeClose = (event: BeforeUnloadEvent) => { if (Object.values(docsRef.current).some(doc => doc.body !== doc.savedBody)) { event.preventDefault(); event.returnValue = ''; } };
    window.addEventListener('beforeunload', beforeClose);
    return () => window.removeEventListener('beforeunload', beforeClose);
  }, []);
  const save = useCallback(async (id: string) => {
    const doc = docsRef.current[id];
    if (!doc || doc.saving || doc.body === doc.savedBody || doc.conflict) return;
    const body = doc.body;
    setDocs(previous => ({ ...previous, [id]: { ...previous[id], saving: true, error: undefined } }));
    try {
      const result = await call<Section>('section_save', { id, body, expectedVersion: doc.section.version });
      setDocs(previous => ({ ...previous, [id]: { ...previous[id], section: result, savedBody: body, saving: false, error: undefined, conflict: false, recovery: false } }));
    } catch (error) {
      const conflict = error instanceof StationError && /CONFLICT|VERSION/i.test(error.code);
      setDocs(previous => ({ ...previous, [id]: { ...previous[id], saving: false, error: errorText(error), conflict } }));
    }
  }, []);
  async function prepareSource(id: string) {
    const source = docsRef.current[id];
    if (!source || source.recovery || source.conflict) throw new Error('请先确认恢复的草稿或处理文件冲突，当前输入仍保留。');
    if (source.body !== source.savedBody) {
      await save(id);
      await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
      if (docsRef.current[id].body !== docsRef.current[id].savedBody) throw new Error('正文尚未保存，请先处理保存提示。当前选句与问题仍保留。');
    }
  }
  useEffect(() => {
    if (!nativeAvailable) return;
    const subscription = getCurrentWindow().onCloseRequested(async event => {
      const drafts = Object.entries(docsRef.current).filter(([, doc]) => doc.body !== doc.savedBody);
      const hasInput = drafts.length || Object.values(questionDrafts.current).some(Boolean) || materialDraft.current.body || materialDraft.current.title || hasExerciseInput();
      try { await fontRef.current.flush(); } catch (error) { event.preventDefault(); notify(`字号尚未保存，窗口已保留：${errorText(error)}`); return; }
      try { await independentRef.current.flush(); } catch (error) { event.preventDefault(); notify(`独立探索草稿尚未保存，窗口已保留：${errorText(error)}`); return; }
      if (!hasInput) return;
      try {
        await Promise.all(Object.entries(docsRef.current).filter(([, doc]) => doc.draftLoaded !== false).map(([id, doc]) => call('draft_save', { key: `section:${id}`, data: { body: doc.body, question: questionDrafts.current[id] || '' } })));
        if (Object.entries(docsRef.current).some(([id, doc]) => unbackedSectionHasInput(doc.draftLoaded, doc.body, doc.savedBody, questionDrafts.current[id] || ''))) throw new Error('小节草稿尚未读取成功，当前输入仍保留。');
        await flushExerciseDrafts();
        if (workspaceDraftLoaded.current) await call('draft_save', { key: 'workspace', data: { importTitle: materialDraft.current.title, importBody: materialDraft.current.body } }); else if (materialDraft.current.body || materialDraft.current.title) throw new Error('导入草稿尚未读取成功，当前输入仍保留。');
      } catch (error) { event.preventDefault(); notify(`草稿还未保存，窗口已保留：${errorText(error)}`); return; }
      await Promise.all(drafts.filter(([, doc]) => !doc.recovery).map(([id]) => save(id)));
      await new Promise<void>(resolve => requestAnimationFrame(() => resolve()));
      if (Object.values(docsRef.current).some(doc => doc.body !== doc.savedBody && !doc.recovery)) { event.preventDefault(); notify('仍有草稿未保存，请先解决保存提示。窗口已保留。'); }
      // Tauri awaits this handler, then destroys the window unless prevented.
    });
    return () => { void subscription.then(unlisten => unlisten()); };
  }, [save, notify]);
  const changeBody = useCallback((id: string, body: string) => {
    setDocs(previous => ({ ...previous, [id]: { ...previous[id], body } }));

  }, []);
  useEffect(() => {
    const timers = Object.entries(docs).filter(([, doc]) => !doc.saving && !doc.error && !doc.recovery && doc.body !== doc.savedBody).map(([id]) => setTimeout(() => { void save(id); }, 1000));
    return () => timers.forEach(clearTimeout);
  }, [docs, save]);
  useEffect(() => {
    if (!nativeAvailable || !activeId || !current || current.draftLoaded === false) return;
    const timer = setTimeout(() => { void call('draft_save', { key: `section:${activeId}`, data: { body: current.body, question } }).then(() => clearDraftError(`section:${activeId}`)).catch(error => reportDraftError(`section:${activeId}`, error)); }, 600);
    return () => clearTimeout(timer);
  }, [activeId, current?.body, current?.draftLoaded, question]);
  useEffect(() => {
    const key = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key === 's') { event.preventDefault(); void save(activeId); }
      if (event.key === 'Escape') { setSelection(''); setImporting(false); setSettings(false); setCardDraft(null); }
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [activeId, save]);
  useEffect(() => { const area = chatEnd.current?.parentElement; if (area) area.scrollTop = area.scrollHeight; }, [session.messages.length, busyId]);

  async function openLinkedExploration() {
    if (!activeId) { setLinkedError('先打开一个小节，或关闭关联开始独立探索。'); return; }
    const sequence = ++linkedSequence.current; setLinkedOpening(true); setLinkedError('');
    try { const result = await call<{ session: LearningSession; context: RequestContext }>('exploration_open', { sectionId: activeId, associateCurrentSection: true }); assertExplorationScope(result, true, activeId); if (sequence === linkedSequence.current) setSessions(old => ({ ...old, [result.session.sectionId]: result.session })); }
    catch (error) { if (sequence === linkedSequence.current) setLinkedError(errorText(error)); }
    finally { if (sequence === linkedSequence.current) setLinkedOpening(false); }
  }
  useEffect(() => { if (panel === 'explore' && associateSection) void openLinkedExploration(); }, [panel, associateSection, activeId]);
  useEffect(() => {
    const sequence = ++contextSequence.current;
    if (!session.id) { setContextView({ status: 'idle' }); return; }
    if (busyId) { setContextView(old => old.sessionId === session.id && old.actual ? old : { status: 'loading', sessionId: session.id }); return; }
    if (!independentMode && current && current.body !== current.savedBody) { setContextView({ status: 'error', sessionId: session.id, error: '正文有未保存改动，保存后会重新核对来源。' }); return; }
    setContextView({ status: 'loading', sessionId: session.id });
    const timer = setTimeout(() => { void call<RequestContext>('context_load', { sessionId: session.id, sectionId: session.sectionId, selection: requestSelection }).then(context => { if (sequence === contextSequence.current) setContextView({ status: 'ready', sessionId: session.id, context }); }).catch(error => { if (sequence === contextSequence.current) setContextView({ status: 'error', sessionId: session.id, error: errorText(error) }); }); }, 150);
    return () => clearTimeout(timer);
  }, [session.id, requestSelection, current?.body, current?.savedBody, current?.section.version, independentMode, busyId, contextRetry]);
  function updateComposer(value: string) { if (independentMode) independent.setQuestion(value); else setQuestion(value); }
  function updateTargetSession(result: LearningSession, independentTarget: boolean) { if (independentTarget) independentRef.current.setSession(result); else setSessions(old => ({ ...old, [result.sectionId]: result })); }
  async function refreshAfterGeneration(target: LearningSession, independentTarget: boolean) {
    const results = await Promise.allSettled([call<LearningSession>('session_refresh', { sectionId: target.sectionId, sessionId: target.id }), call<Pick<Bootstrap, 'tasks' | 'notices'>>('activity_refresh')]);
    if (results[0].status === 'fulfilled') updateTargetSession(results[0].value, independentTarget); else notify(`学习记录刷新失败，已有内容仍保留：${errorText(results[0].reason)}`);
    if (results[1].status === 'fulfilled') { const activity = results[1].value; setBoot(old => ({ ...old, ...activity })); } else notify(`待办与通知尚未刷新：${errorText(results[1].reason)}`);
  }
  async function saveLocalQuestion() {
    const value = composerQuestion.trim(); if (!value || localQuestionSaving) return;
    if (!canCompose || !session.id) { setAiError('学习会话尚未就绪，请稍候或重试打开。'); return; }
    const target = session; const detached = independentMode; setLocalQuestionSaving(true);
    try { const result = await call<LearningSession>('question_save', { sectionId: target.sectionId, sessionId: target.id, question: value, selection: requestSelection }); updateTargetSession(result, detached); if (detached) { if (independentRef.current.question.trim() === value) independentRef.current.setQuestion(''); } else if (activeIdRef.current === target.sectionId) setQuestion(old => old.trim() === value ? '' : old); notify('困惑已保存在学习记录中，未请求AI。'); }
    catch (error) { setAiError(`困惑尚未保存，输入仍保留：${errorText(error)}`); }
    finally { setLocalQuestionSaving(false); }
  }
  async function ask(preset?: string) {
    const value = (preset ?? composerQuestion).trim();
    if (!value || busyRef.current) return;
    if (!canCompose || !session.id) { setAiError('学习会话尚未就绪，请重试打开，问题仍保留。'); return; }
    if (!modelReady) { notify(modelBlocked ? '此模型用途受限，请查看设置中的说明或配置获准使用的自定义服务。' : '请先保存你自己的模型连接配置。'); setSettings(true); return; }
    const target = session; const detached = independentMode; const selected = requestSelection;
    const id = makeId(); busyRef.current = id; setBusyId(id); setPartial(null); setAiError('');
    try {
      if (!detached) await prepareSource(target.sectionId);
      const context = await call<RequestContext>('context_load', { sessionId: target.id, sectionId: target.sectionId, selection: selected });
      setContextView({ status: 'loading', sessionId: target.id, context });
      const result = await call<LearningSession>('session_ask', { sectionId: target.sectionId, sessionId: target.id, mode: panel, question: value, selection: selected, operationId: id });
      updateTargetSession(result, detached);
      if (detached) { if (independentRef.current.question.trim() === value) independentRef.current.setQuestion(''); } else if (activeIdRef.current === target.sectionId) setQuestion(old => old.trim() === value ? '' : old);
    } catch (error) { if (visibleSessionRef.current === target.id) setAiError(modelFailureMessage(error, 'generation')); else notify(`另一会话生成未完成，输入仍保留：${modelFailureMessage(error, 'generation')}`); }
    finally { await refreshAfterGeneration(target, detached); if (busyRef.current === id) { busyRef.current = ''; setBusyId(''); } }
  }
  async function cancel() {
    const id = busyRef.current; if (!id) return;
    try { await call('operation_cancel', { operationId: id }); notify('已请求停止生成，已保存的对话仍保留。'); }
    catch (error) { notify(`停止请求未确认：${errorText(error)}`); }
    finally { busyRef.current = ''; setBusyId(''); }
  }
  async function generateExercise() {
    if (!current || busyRef.current) return;
    if (!modelReady) { notify(modelBlocked ? '该模型用途受限，请查看设置说明。' : '请先保存模型连接配置。'); setSettings(true); return; }
    const id = makeId(); busyRef.current = id; setBusyId(id); setPartial(null); setAiError('');
    const sectionId = activeId;
    try { await prepareSource(sectionId); const result = await call<LearningSession>('exercise_generate', { sectionId, sessionId: session.id, selection, operationId: id }); if (busyRef.current === id) setSessions(old => ({ ...old, [sectionId]: result })); }
    catch (error) { if (busyRef.current === id) setAiError(modelFailureMessage(error, 'generation')); }
    finally { await refreshAfterGeneration(session, false); if (busyRef.current === id) { busyRef.current = ''; setBusyId(''); } }
  }
  async function submitAnswer(exercise: Exercise, answer: string) {
    const result = await call<LearningSession>('exercise_answer', { sectionId: activeId, sessionId: session.id, exerciseId: exercise.id, entryId: exercise.entryId, answer });
    setSessions(old => ({ ...old, [activeId]: result }));
  }
  async function saveReason(exercise: Exercise, reason: string) {
    const result = await call<LearningSession>('mistake_save', { sectionId: activeId, exerciseId: exercise.id, question: exercise.prompt, reason });
    setSessions(old => ({ ...old, [activeId]: result })); notify('错因已保存，下次可按情境找回。');
  }
  async function loadReview(query = '') {
    setReviewLoading(true); setReviewError('');
    try { setCards(await call<ReviewCard[]>('review_list', { query })); }
    catch (error) { setReviewError(errorText(error)); }
    finally { setReviewLoading(false); }
  }
  async function exportCards() {
    try { const result = await call<{ path: string }>('cards_export', { format: 'csv', sectionId: activeId }); notify(`当前小节的 CSV 已导出：${result.path}`); }
    catch (error) { setReviewError(errorText(error)); }
  }
  async function createTask(event: React.FormEvent) {
    event.preventDefault(); if (!taskTitle.trim() || taskBusy) return;
    setTaskBusy(true); setTaskError('');
    try { const task = await call<Task>('task_create', { title: taskTitle.trim(), dueAt: taskReminder && taskDue ? new Date(taskDue).toISOString() : null, reminderEnabled: taskReminder, sectionId: activeId || null }); setBoot(previous => ({ ...previous, tasks: [...previous.tasks, task] })); setTaskTitle(''); setTaskDue(''); setTaskReminder(false); }
    catch (error) { setTaskError(errorText(error)); }
    finally { setTaskBusy(false); }
  }
  async function toggleTask(task: Task) {
    try { const next = await call<Task>('task_update', { id: task.id, completed: !task.completed }); setBoot(previous => ({ ...previous, tasks: previous.tasks.map(item => item.id === task.id ? next : item) })); }
    catch (error) { setTaskError(errorText(error)); }
  }
  async function toggleReminder(task: Task) {
    try { const next = await call<Task>('task_reminder', { id: task.id, reminderEnabled: !task.reminderEnabled }); setBoot(old => ({ ...old, tasks: old.tasks.map(item => item.id === task.id ? next : item) })); }
    catch (error) { setTaskError(errorText(error)); }
  }
  function selectText() {
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed && document.querySelector('.reading-content')?.contains(selection.anchorNode)) setSelection(selection.toString().trim().slice(0, 5000));
  }
  function currentDraftData(key: string): UiDraft['data'] {
    if (key === 'workspace') return { importTitle: materialDraft.current.title, importBody: materialDraft.current.body };
    const id = key.slice('section:'.length);
    return { body: docsRef.current[id]?.body || '', question: questionDrafts.current[id] || '' };
  }
  function markDraftLoaded(key: string, loaded: boolean) {
    if (key === 'workspace') workspaceDraftLoaded.current = loaded;
    else { const id = key.slice('section:'.length); setDocs(old => old[id] ? ({ ...old, [id]: { ...old[id], draftLoaded: loaded } }) : old); }
  }
  async function retryBackup(key: string) {
    markDraftLoaded(key, false);
    try {
      const saved = await call<UiDraft | null>('draft_get', { key });
      if (saved && draftInputsDiffer(key, currentDraftData(key), saved.data)) { setBackupConflict(saved); return; }
      markDraftLoaded(key, true); clearDraftError(key);
    } catch (error) { reportDraftError(key, error); }
  }
  async function resolveBackup(useSaved: boolean) {
    if (!backupConflict) return;
    const { key, data } = backupConflict;
    if (useSaved) {
      if (key === 'workspace') materialDraft.current = { title: data.importTitle || '', body: data.importBody || '' };
      else {
        const id = key.slice('section:'.length);
        questionDrafts.current[id] = data.question || '';
        if (activeIdRef.current === id) { setQuestion(data.question || ''); setEditing(false); }
        setDocs(old => old[id] ? ({ ...old, [id]: { ...old[id], body: data.body ?? old[id].body, recovery: data.body !== undefined && data.body !== old[id].savedBody, draftLoaded: true } }) : old);
        setEditedIds(old => { const next = new Set(old); next.delete(id); return next; });
      }
    }
    markDraftLoaded(key, true); setBackupConflict(null); clearDraftError(key);
    if (!useSaved) { try { await call('draft_save', { key, data: currentDraftData(key) }); } catch (error) { reportDraftError(key, error); } }
    if (key === 'workspace' && useSaved) setImporting(true);
  }
  function openPanel(mode: Panel, preset?: string) { setPanel(mode); setRightbar(true); if (preset) { if (mode === 'explore' && !associateSection) independent.setQuestion(preset); else setQuestion(preset); } setTimeout(() => questionRef.current?.focus(), 0); }

  return <div style={{ '--content-font-size': `${font.fontSize}px`, '--ui-font-scale': font.fontSize / 16 } as React.CSSProperties} className={`app-shell ${!sidebar ? 'sidebar-collapsed' : ''} ${!rightbar || view !== 'lesson' ? 'rightbar-collapsed' : ''} ${immersive && rightbar && view === 'lesson' ? 'immersive' : ''}`}>
    {isDemo && <div className="demo-banner">开发预览 · 示例材料，数据仅保留在内存 · 未连接本机服务或 AI</div>}
    <aside className="sidebar">
      <div className="brand"><span className="brand-mark"><BookOpen size={18} /></span><div>学习站<span>LEARNING STATION</span></div><button className="icon-button sidebar-collapse" title="收起课程栏" aria-label="收起课程栏" onClick={() => setSidebar(false)}><PanelLeftClose size={17} /></button></div>
      <button className="add-material" onClick={() => setImporting(true)}><Plus size={17} /> 打开我的学习材料</button>
      <div className="nav-label">我的课程 <span>{boot.courses.length.toString().padStart(2, '0')}</span></div>
      <label className="sidebar-search"><Search size={15} /><input aria-label="查找课程或小节" value={filter} onChange={event => setFilter(event.target.value)} placeholder="查找小节" /><kbd>⌕</kbd></label>
      <div className="course-list">
        {loading && <div className="sidebar-status"><LoaderCircle className="spin" size={15} /> 正在打开本地课程</div>}
        {!loading && !boot.courses.length && <div className="sidebar-empty">课程会在这里整齐排好。<br />先打开一份 Markdown 或粘贴材料。</div>}
        {boot.courses.map(course => <div className="course" key={course.id}>
          <div className="course-title"><ChevronDown size={13} /><FolderOpen size={15} /><span>{course.title}</span></div>
          {course.sections.filter(section => `${course.title} ${section.title}`.toLowerCase().includes(filter.toLowerCase())).map((section, index) => <button className={`section-link ${activeId === section.id && view === 'lesson' ? 'active' : ''}`} key={section.id} onClick={() => { void openSection(section.id); }}><span className="section-index">{String(index + 1).padStart(2, '0')}</span><span>{section.title.replace(/^\d+\s*[·.、-]\s*/, '')}</span>{docs[section.id]?.body !== docs[section.id]?.savedBody && <span className="draft-dot" title="有未保存内容" />}</button>)}
        </div>)}
      </div>
      <div className="sidebar-bottom">
        <button className={`bottom-link ${view === 'review' ? 'active' : ''}`} onClick={() => { switchView('review'); void loadReview(); }}><Layers2 size={17} /><span>复习与历史</span><ArrowRight size={14} /></button>
        <button className={`bottom-link ${view === 'tasks' ? 'active' : ''}`} onClick={() => switchView('tasks')}><ListTodo size={17} /><span>学习待办</span>{boot.tasks.filter(task => !task.completed).length > 0 && <span className="count">{boot.tasks.filter(task => !task.completed).length}</span>}</button>
        <div className="sidebar-divider" />
        <button className="bottom-link" onClick={() => setSettings(true)}><Settings2 size={17} /><span>设置与连接</span><span className={`connection-dot ${modelReady ? 'connected' : ''}`} /></button>
        <div className="local-status"><span className="tiny-dot" /> {isDemo ? '开发预览 · 未写入文件' : '材料保存在这台电脑'}</div>
      </div>
    </aside>

    <main className="main-area">
      <header className="topbar">
        <div className="breadcrumbs">{!sidebar && <button className="icon-button" aria-label="展开课程栏" onClick={() => setSidebar(true)}><PanelLeftOpen size={18} /></button>}<span>{view === 'lesson' ? (activeCourse?.title || '我的学习空间') : view === 'review' ? '复习与历史' : '学习待办'}</span><ChevronRight size={12} /><span className="breadcrumb-current">{view === 'lesson' ? '阅读与理解' : view === 'review' ? '把学过的串起来' : '接着完成下一步'}</span></div>
        <div className="topbar-actions"><button className={`icon-button notice-button ${boot.notices.some(notice => !notice.read) ? 'has-notices' : ''}`} aria-label="通知" title="通知" onClick={() => setNotifications(!notifications)}><Clock3 size={17} /></button>{view === 'lesson' && <button className="icon-button" aria-label={rightbar ? '收起学习助手' : '打开学习助手'} onClick={() => setRightbar(!rightbar)}>{rightbar ? <PanelRightClose size={18} /> : <PanelRightOpen size={18} />}</button>}</div>
      </header>
      {Object.entries(draftErrors).map(([key, message]) => <div className="connection-banner" role="status" key={key}><CircleHelp size={16} /><span>{key === 'workspace' ? '导入草稿' : '小节草稿'}备份尚未完成：{message} 当前输入仍保留在窗口中。</span><button onClick={() => { void retryBackup(key); }}>重试读取原草稿</button></div>)}
      {bootError && <div className="connection-banner" role="status"><CircleHelp size={16} /><span>{bootError}</span><button onClick={() => { void load(); }}>重新连接</button></div>}
      {notifications && <div className="notice-popover"><div className="popover-heading">学习通知<button className="icon-button" aria-label="关闭通知" onClick={() => setNotifications(false)}><X size={15} /></button></div>{boot.notices.length ? boot.notices.map(notice => <article key={notice.id}><strong>{notice.title}</strong><p>{notice.body || '学习待办已到期。'}</p>{!notice.read && <button className="quiet-button" onClick={async () => { try { await call('notice_read', { id: notice.id }); setBoot(old => ({ ...old, notices: old.notices.map(item => item.id === notice.id ? { ...item, read: true } : item) })); } catch (error) { notify(errorText(error)); } }}>标为已读</button>}</article>) : <div className="subtle-empty">还没有通知。到期的学习待办会出现在这里。</div>}</div>}

      <div className="lesson-view" hidden={view !== 'lesson'}>
        {opening && !current ? <div className="center-state"><LoaderCircle className="spin" size={25} /><p>正在打开小节…</p></div> : current ? <>
          <div className="document-top"><div className="document-kicker"><span className="eyebrow">READ · THINK · CONNECT</span><div className={`save-status ${current.error ? 'status-error' : ''}`} aria-live="polite">{current.saving ? <><LoaderCircle size={12} className="spin" /> 保存中</> : current.error ? <><CircleHelp size={12} /> {current.conflict ? '文件有新版本' : '尚未保存'}</> : current.body !== current.savedBody ? <><span className="draft-dot" /> 本机草稿 · 等待保存</> : <><CheckCheck size={13} /> {isDemo ? '预览内容 · 仅在内存' : '已保存到本机'}</>}</div></div>
            <div className="document-title-row"><h1>{current.section.title.replace(/^\d+\s*[·.、-]\s*/, '')}</h1><button className="icon-button" title="查看来源" aria-label="查看来源" onClick={() => { const details = document.querySelector<HTMLDetailsElement>('.document-meta'); if (details) details.open = !details.open; }}><MoreHorizontal size={20} /></button></div>
            <div className="document-toolbar"><div className="segmented"><button className={!editing ? 'selected' : ''} onClick={() => setEditing(false)}><BookOpen size={14} />阅读</button><button className={editing ? 'selected' : ''} onClick={() => { if (!editedIds.has(activeId)) editorStartedAt.current[activeId] = performance.now(); setEditedIds(old => new Set([...old, activeId])); setEditing(true); }}><PencilLine size={14} />编辑</button></div><span className="toolbar-hint">{editing ? '直接编辑正文 · ⌘S 保存' : '选中一句话，继续追问'}</span><span className="word-count">{current.body.split(/\s+/).filter(Boolean).length} 词</span></div>
            <details className="document-meta"><summary><FileText size={12} />来源与文件信息 <ChevronDown size={12} /></summary><p>{current.section.sourcePath}</p><p>版本 {current.section.version} · 顶部元数据由系统维护，正文可自由编辑。</p></details>
            {current.recovery && <div className="recovered-draft"><strong>已找回上次未写入文件的草稿</strong><p>当前显示你的草稿，原文件保持原样。确认后保存，或展开来源检查外部文件。</p><button className="quiet-button" onClick={() => { void save(activeId); }}>保存这份草稿</button><button className="quiet-button" onClick={async () => { try { setComparison(await call<Section>('section_open', { id: activeId })); } catch (error) { notify(errorText(error)); } }}>对照原文件</button></div>}
            {current.error && <div className="inline-error"><strong>{current.conflict ? '外部文件已更新，你的编辑仍在这里。' : '这次编辑还没有写入文件。'}</strong><p>{current.error}</p><div className="button-row">{current.conflict && <button onClick={async () => { try { setComparison(await call<Section>('section_open', { id: activeId })); } catch (error) { notify(errorText(error)); } }}>查看外部新版本</button>}{!current.conflict && <button onClick={() => { void save(activeId); }}>重新保存</button>}<button onClick={async () => { try { const result = await call<Section>('section_import', { title: `${current.section.title}（我的草稿）`, body: current.body }); notify('已另存一份草稿，原文件保留。'); setBoot(old => ({ ...old, courses: old.courses.map(course => course.id === result.courseId ? { ...course, sections: [...course.sections, { id: result.id, title: result.title }] } : course) })); setDocs(old => ({ ...old, [result.id]: { section: result, body: result.body, savedBody: result.body, saving: false, draftLoaded: result.draftReady !== false } })); setActiveId(result.id); } catch (error) { notify(errorText(error)); } }}>另存为新小节</button></div></div>}
          </div>
          <div className="document-scroll" onMouseUp={selectText} onKeyUp={selectText}>
            {!editing && <div className="reading-content" tabIndex={0}><Markdown text={current.body} className={current.body.split('\n')[0].replace(/^#\s+/, '').trim().toLowerCase() === current.section.title.replace(/^\d+\s*[·.、-]\s*/, '').trim().toLowerCase() ? 'hide-redundant-title' : ''} /><div className="reading-end"><span /><BookOpen size={15} /><span /></div></div>}
            {Object.entries(docs).filter(([id]) => editedIds.has(id)).map(([id, doc]) => <div key={id} className="document-editor" hidden={!editing || activeId !== id}><Suspense fallback={<div className="muted small">正在打开编辑器…</div>}><Editor initial={doc.body} onReady={() => { const started = editorStartedAt.current[id]; if (started !== undefined) { recordMetric('editor-ready', performance.now() - started); delete editorStartedAt.current[id]; } }} onChange={body => changeBody(id, body)} onSelection={setSelection} onSave={() => { void save(id); }} /></Suspense></div>)}
          </div>

        </> : <div className="welcome"><div className="welcome-symbol"><BookOpen size={32} strokeWidth={1.4} /></div><span className="eyebrow">YOUR OWN LEARNING SPACE</span><h1>从一小节开始，<br />把一个问题读明白。</h1><p>打开你的英语材料，边读边问。<br />小节、讲解和学过的错因，会自然留在一起。</p><button className="primary-button" onClick={() => setImporting(true)}><Plus size={16} />打开或粘贴我的材料</button><span className="welcome-support">支持 Markdown 与纯文本 · 原文保存在本机</span><button className="template-button" onClick={() => { if (!materialDraft.current.body) materialDraft.current = { title: "英语学习 · 每天的一小步（原创示例）", body: sampleBody }; else notify("已保留你未完成的导入草稿，请先保存它。"); setImporting(true); }}><BookOpen size={14} />使用英语学习模板<span>原创示例 · 可编辑 · 非 AI 生成</span></button>{openError && <div className="inline-error">{openError}<button onClick={() => { void openSection(activeId); }}>重试打开</button></div>}<div className="welcome-steps"><span><b>01</b> 读自己的材料</span><span><b>02</b> 随时问与练</span><span><b>03</b> 留下值得复习的</span></div></div>}
      </div>

      {view === 'review' && <div className="utility-page"><button className="text-back" onClick={() => switchView('lesson')}><ArrowLeft size={14} />回到小节</button><span className="eyebrow">CONNECT WHAT YOU KNOW</span><h1>学过的，下次用得上。</h1><p className="page-description">输入下一次考试或任务，找回相关选句与错因。检索来自你真实保存的学习记录。</p><form className="review-search" onSubmit={event => { event.preventDefault(); void loadReview(reviewQuery); }}><Search size={18} /><input aria-label="复习情境" placeholder="例如：雅思阅读、转折关系、写作论证…" value={reviewQuery} onChange={event => setReviewQuery(event.target.value)} /><button className="primary-button" type="submit" disabled={reviewLoading}>{reviewLoading ? <LoaderCircle size={15} className="spin" /> : <ArrowRight size={16} />}</button></form><div className="section-heading"><h2>我的复习卡 <span>{cards.length}</span></h2><button className="quiet-button" disabled={!cards.length} onClick={() => { void exportCards(); }}><ArrowDownToLine size={14} />导出当前小节 CSV</button></div><p className="muted small">CSV 可自行导入 Anki；当前未连接 Anki 同步。</p>{reviewError && <div className="inline-error" role="alert">{reviewError}<button onClick={() => { void loadReview(reviewQuery); }}>重试</button></div>}{!reviewLoading && !cards.length && !reviewError && <div className="empty-state"><Layers2 size={30} strokeWidth={1.2} /><h3>{reviewQuery ? '还没有相关的历史记录' : '值得记住的内容，会留在这里'}</h3><p>{reviewQuery ? '换一个关键词，或先在小节里保存选句和错因。' : '阅读时选中一句话，选择“留作复习”。错题反馈里的错因也可以保存。'}</p><button className="quiet-button" onClick={() => switchView('lesson')}>回到小节 <ArrowRight size={14} /></button></div>}<div className="review-cards">{cards.map(card => <article className="review-card" key={card.id}><span className="card-label"><Layers2 size={13} />回忆一下</span><Markdown text={card.front} />{showBack[card.id] && <div className="card-answer"><Markdown text={card.back || '这张卡片还没有补充解释。'} /></div>}<div className="card-footer"><span>{card.source || '来自我的小节'}</span><button title="回到原小节" onClick={() => { void openSection(card.sectionId); }}><FileText size={12} />来源小节</button><button onClick={() => setShowBack(old => ({ ...old, [card.id]: !old[card.id] }))}>{showBack[card.id] ? '收起解释' : '查看解释'}<ChevronDown size={13} /></button></div></article>)}</div></div>}
      {view === 'tasks' && <div className="utility-page"><button className="text-back" onClick={() => switchView('lesson')}><ArrowLeft size={14} />回到小节</button><span className="eyebrow">ONE SMALL NEXT STEP</span><h1>给下次学习，留一步。</h1><p className="page-description">记下一个具体动作，让开始变得容易。到期状态由本机服务管理。</p><form className="task-form" onSubmit={event => { void createTask(event); }}><label>下一步做什么<input value={taskTitle} onChange={event => setTaskTitle(event.target.value)} placeholder="例如：再读一遍今天的转折句" required maxLength={200} /></label><div className="task-form-bottom"><div className="task-reminder-fields"><label className="checkbox-label"><input type="checkbox" checked={taskReminder} onChange={event => setTaskReminder(event.target.checked)} />到时间提醒我</label>{taskReminder && <label>提醒时间<input type="datetime-local" value={taskDue} onChange={event => setTaskDue(event.target.value)} required /></label>}</div><button className="primary-button" disabled={taskBusy || !taskTitle.trim()}>{taskBusy ? <LoaderCircle className="spin" size={15} /> : <Plus size={15} />}添加待办</button></div>{taskReminder && <p className="muted small">应用退出或电脑休眠时无法准点提醒；重开后会补入通知。飞书提醒未启用。</p>}</form>{taskError && <div className="inline-error" role="alert">{taskError}</div>}<div className="section-heading"><h2>学习待办 <span>{boot.tasks.filter(task => !task.completed).length}</span></h2></div>{!boot.tasks.length && <div className="empty-state"><ListTodo size={30} strokeWidth={1.2} /><h3>现在没有待办</h3><p>今天不必安排很多。一个能完成的小动作就够了。</p></div>}<div className="task-list">{boot.tasks.map(task => <div className={`task-item ${task.completed ? 'completed' : ''}`} key={task.id}><button className="task-check" aria-label={task.completed ? '标为未完成' : '完成待办'} onClick={() => { void toggleTask(task); }}>{task.completed && <Check size={14} />}</button><div><span>{task.title}</span>{task.dueAt && <small><Clock3 size={11} />{new Date(task.dueAt).toLocaleString('zh-CN', { month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit' })} · {task.reminderEnabled ? '提醒已开启' : '提醒未开启'}</small>}</div>{task.dueAt && !task.completed && <button className="task-reminder-toggle" onClick={() => { void toggleReminder(task); }}>{task.reminderEnabled ? '关闭提醒' : '开启提醒'}</button>}</div>)}</div></div>}
      <footer className="statusbar"><span><span className={`tiny-dot ${nativeAvailable ? '' : 'offline'}`} />{isDemo ? '开发预览' : nativeAvailable ? '本机工作空间' : '等待本机连接'}</span><span>{dirtyCount ? `${dirtyCount} 份草稿待保存` : '每一点理解，都值得留下'}</span><span>⌘ S 保存</span></footer>
    </main>

    {view === 'lesson' && <aside className="assistant-panel"><div className="assistant-heading"><span className="assistant-mark"><Sparkles size={17} /></span><div><h2>一起读明白</h2><p>顺着你的问题，往前一步</p></div><button className="expand-assistant" aria-label={immersive ? '恢复三栏阅读' : '全展开学习助手'} aria-pressed={immersive} onClick={toggleImmersive}>{immersive ? <Minimize2 size={15} /> : <Maximize2 size={15} />}<span>{immersive ? '恢复阅读' : '全展开'}</span></button><button className="icon-button" aria-label="收起学习助手" onClick={() => { setImmersive(false); setRightbar(false); }}><PanelRightClose size={17} /></button></div><div className="assistant-tabs" role="tablist" aria-label="学习方式">{(['explain', 'explore', 'practice'] as Panel[]).map(mode => <button role="tab" aria-selected={panel === mode} key={mode} className={panel === mode ? 'selected' : ''} onClick={() => setPanel(mode)}>{mode === 'explain' ? '讲解' : mode === 'explore' ? '探索' : '练习'}</button>)}</div>
      <div className="assistant-scroll" ref={assistantScroll}>
        {panel === 'explore' && <div className="exploration-scope"><label className="checkbox-label"><input type="checkbox" checked={associateSection} disabled={!!busyId} onChange={event => { setAssociateSection(event.target.checked); setAiError(''); setPartial(null); }} />关联当前小节</label><p>{associateSection ? '关联探索：仅使用服务端核对的来源。' : '独立探索：不带入当前小节、选句或其他小节历史。'}</p>{busyId && <p>生成中；请先停止，再切换关联方式。</p>}{(independentMode ? independent.loading : linkedOpening) && <p role="status">正在打开探索会话…</p>}{(independentMode ? independent.error : linkedError) && <div className="inline-error" role="alert">{independentMode ? independent.error : linkedError}<button onClick={() => { if (independentMode) void independent.retry(); else void openLinkedExploration(); }}>重试打开探索</button></div>}</div>}
        <RequestContextView value={contextView.sessionId && contextView.sessionId !== session.id ? { status: 'loading' } : contextView} independent={independentMode} onRetry={() => setContextRetry(old => old + 1)} />
        {!independentMode && selection && <div className="selection-tray"><div className="selected-quote"><Highlighter size={13} /><span>当前选句 · 发送前会核对上下文</span><button className="icon-button" aria-label="取消选句" onClick={() => setSelection('')}><X size={13} /></button></div><blockquote>{selection}</blockquote><div className="selection-actions"><button onClick={() => openPanel('explain', '请帮我理解这句话的含义和表达。')}><MessageCircle size={12} />问一句</button><button onClick={() => openPanel('practice')}><Sparkles size={12} />考考我</button><button onClick={() => setCardDraft({ front: selection, back: '' })}><Layers2 size={12} />留作复习</button></div></div>}
        {independentMode && independent.draftError && <div className="inline-error" role="alert">独立探索草稿尚未备份：{independent.draftError}{independent.conflict !== null ? <><p>旧问题：{independent.conflict || '（空）'}</p><button onClick={() => independent.resolveDraft(true)}>恢复旧问题</button><button onClick={() => independent.resolveDraft(false)}>保留当前问题并备份</button></> : <button onClick={() => { void independent.retryDraft(); }}>重试读取探索草稿</button>}</div>}
        {independentMode && independent.recovery && <details className="recovery-message"><summary>独立探索上次生成未完成</summary><p>{independent.recovery.message || '已有记录仍保留。'}</p><Markdown text={independent.recovery.content} /></details>}
        {!independentMode && boot.recovery && <details className="recovery-message"><summary>上次生成未完成 · 查看保留内容<ChevronDown size={12} /></summary><p>{boot.recovery.message || '应用关闭时生成尚未完成，已保存内容仍保留。'}</p>{boot.recovery.content ? <Markdown text={boot.recovery.content} /> : <p>尚未收到正文。可以重新发送问题。</p>}</details>}
        {!modelReady && <div className="model-prompt"><span className="model-prompt-icon"><Sparkles size={16} /></span><div><strong>{modelBlocked ? '当前模型用途受限' : '连接自己的模型，开始对话'}</strong><p>{modelBlocked ? '可在设置中查看限制或配置获准使用的自定义服务。' : '可手动填写服务地址、协议和模型名称。'}</p><button onClick={() => setSettings(true)}>设置模型连接 <ArrowRight size={13} /></button></div></div>}
        {panel !== 'practice' && <>
          {!session.messages.length && <div className="assistant-empty"><div className="little-line" /><h3>{panel === 'explore' ? '好问题，可以从这里展开。' : '哪里卡住了？'}</h3><p>{panel === 'explore' ? '从一个想法出发，比较、追问，再回到自己的理解。' : '词义、句子关系，或一整段的意思，都可以慢慢问。'}</p><div className="starter-questions"><button disabled={!canCompose} onClick={() => { updateComposer(independentMode ? '我想独立探索一个问题，请先帮我澄清。' : '请带我梳理这一小节的核心意思，先从一个问题开始。'); questionRef.current?.focus(); }}>{independentMode ? '澄清一个独立问题' : '带我梳理这一小节'} <ArrowRight size={13} /></button><button disabled={!canCompose} onClick={() => { updateComposer(independentMode ? '请从不同角度分析我接下来提出的问题。' : '这篇材料里，有哪些容易理解错的表达？'); questionRef.current?.focus(); }}>{independentMode ? '从不同角度展开' : '找找容易理解错的表达'} <ArrowRight size={13} /></button></div></div>}
          <div className="messages">{session.messages.slice(-30).map(message => <article className={`message ${message.role}`} key={message.id}><div className="message-label">{message.role === 'user' ? '我的问题' : message.role === 'note' ? <><FileText size={12} />学习记录 · AI整理{message.needsConfirmation && <span className="pending-label">待确认</span>}</> : <><Sparkles size={12} />学习助手</>}</div><Markdown text={message.content} />{message.sources?.length ? <SourceList sources={message.sources} /> : null}{message.role === 'assistant' && <button className="message-save" onClick={() => setCardDraft({ front: selection || '这一小节中值得回忆的问题', back: message.content })}><Plus size={12} />留作复习</button>}</article>)}</div>{session.messages.length > 30 && <p className="muted small">显示最近 30 条。完整记录保存在学习文件中。</p>}
        </>}
        {panel === 'practice' && <div className="practice-panel"><span className="card-label">TRY IT YOURSELF</span><h3>用一道题，检查理解。</h3><p className="muted">围绕{selection ? '当前选句' : '当前小节'}与已记录的困惑生成，不必先听完讲解。</p><button className="primary-button full-width" disabled={!!busyId} onClick={() => { void generateExercise(); }}><Sparkles size={14} />生成一道针对练习</button><button className="manual-exercise-link" disabled={!current} onClick={() => setManualExercise(true)}><PencilLine size={12} />或手动添加一道题</button>{session.exercises.map(exercise => <ExerciseCard key={exercise.id} exercise={exercise} onAnswer={answer => submitAnswer(exercise, answer)} onReason={reason => saveReason(exercise, reason)} />)}</div>}
        {partial && partial.sessionId === session.id && busyId && <article className="message assistant partial-message"><div className="message-label"><Sparkles size={12} />正在回复</div><Markdown text={partial.content} /></article>}{busyId && <div className="generating" role="status"><LoaderCircle size={15} className="spin" /><div>正在等待模型回复…<small>{independentMode ? '独立探索 · 未关联当前小节' : '来源以核对后的实际清单为准'}</small></div><button className="quiet-button" onClick={() => { void cancel(); }}><Square size={11} />停止</button></div>}{aiError && <div className="inline-error" role="alert"><strong>这次没有得到回复</strong><p>{aiError}</p><span>输入仍保留，可以修改后重试。</span></div>}<div ref={chatEnd} />
      </div>
      <div className="composer-area"><form className="composer" onSubmit={event => { event.preventDefault(); void ask(); }}><textarea ref={questionRef} aria-label="向学习助手提问" placeholder={independentMode ? '独立探索，不关联当前小节…' : current ? '写下你的问题…' : '先打开材料，或切换到独立探索'} value={composerQuestion} disabled={!canCompose} rows={3} onChange={event => updateComposer(event.target.value)} onCompositionStart={() => { cancelAnimationFrame(composerCommitFrame.current); composerComposing.current = true; }} onCompositionEnd={() => { composerCommitFrame.current = requestAnimationFrame(() => { composerComposing.current = false; }); }} onBlur={() => { cancelAnimationFrame(composerCommitFrame.current); composerComposing.current = false; }} onKeyDown={event => { const action = composerKeyAction(event.nativeEvent, composerComposing.current); if (action === 'native') return; event.preventDefault(); if (action === 'send') void ask(); }} /><div className="composer-bottom"><button type="button" className="save-question" disabled={!composerQuestion.trim() || localQuestionSaving} onClick={() => { void saveLocalQuestion(); }}>{localQuestionSaving ? <LoaderCircle size={11} className="spin" /> : <Plus size={11} />}保存困惑</button><button type="submit" className="send-button" aria-label={modelReady ? '发送问题' : '配置模型后提问'} disabled={!composerQuestion.trim() || !!busyId}><ArrowUp size={17} /></button></div></form><p className="composer-note">{modelReady ? `${boot.model.model} · ${independentMode ? '独立探索' : '关联来源可展开核对'}` : '可配置自定义模型 · 本地阅读与练习可独立使用'}<br />Enter 发送 · Shift+Enter 换行</p></div>
    </aside>}

    {importing && <ImportDialog draft={materialDraft} onDraftLoaded={loaded => { workspaceDraftLoaded.current = loaded; if (loaded) clearDraftError('workspace'); }} onClose={() => setImporting(false)} onImported={async section => { if (section.draftError) reportDraftError(`section:${section.id}`, section.draftError); if (workspaceDraftLoaded.current) { materialDraft.current = { title: '', body: '' }; if (nativeAvailable) void call('draft_save', { key: 'workspace', data: { importTitle: '', importBody: '' } }).catch(error => reportDraftError('workspace', error)); } setImporting(false); setDocs(old => ({ ...old, [section.id]: { section, body: section.body, savedBody: section.body, saving: false, draftLoaded: section.draftReady !== false } })); setSessions(old => ({ ...old, [section.id]: emptySession(section.id) })); setActiveId(section.id); setQuestion(questionForSection(questionDrafts.current, section.id)); setSelection(''); setAiError(''); switchView('lesson'); setEditing(false); try { setBoot(await call<Bootstrap>('bootstrap')); } catch (error) { notify(errorText(error)); } }} />}
    {settings && <SettingsDialog font={font} model={boot.model} dataPath={boot.dataPath} onClose={() => setSettings(false)} onSaved={model => setBoot(old => ({ ...old, model }))} />}
    {backupConflict && <Dialog title="找回了之前的草稿" description="旧草稿还没有被覆盖。请对照正文与未发送的问题，再选择保留哪份输入。" onClose={() => setBackupConflict(null)}><div className="conflict-compare">{[{ label: '当前窗口输入', data: currentDraftData(backupConflict.key) }, { label: '找回的旧草稿', data: backupConflict.data }].map(item => <div key={item.label}><h3>{item.label}</h3>{backupConflict.key === 'workspace' ? <><h4>小节名称</h4><p>{item.data.importTitle || '（空）'}</p><h4>正文</h4><Markdown text={item.data.importBody || '（空）'} /></> : <><h4>正文</h4><Markdown text={item.data.body || '（空）'} /><h4>未发送的问题</h4><p className="draft-question">{item.data.question || '（空）'}</p></>}</div>)}</div><div className="dialog-footer"><button className="quiet-button" onClick={() => { void resolveBackup(true); }}>使用找回的草稿</button><button className="primary-button" onClick={() => { void resolveBackup(false); }}>保留当前输入并备份</button></div></Dialog>}
    {manualExercise && <ManualExerciseDialog sectionId={activeId} onClose={() => setManualExercise(false)} onSaved={result => { setSessions(old => ({ ...old, [activeId]: result })); setManualExercise(false); notify('手动题目已保存，可以开始作答。'); }} />}
    {comparison && <Dialog title="对照外部的新版本" description="左侧是仍保留的本地编辑，右侧是当前文件。关闭对照后可以继续整理，或将草稿另存为新小节。" onClose={() => setComparison(null)}><div className="conflict-compare"><div><h3>我的编辑</h3><Markdown text={current?.body || ''} /></div><div><h3>外部文件</h3><Markdown text={comparison.body} /></div></div></Dialog>}
    {cardDraft && <CardDialog draft={cardDraft} sectionId={activeId} onClose={() => setCardDraft(null)} onSaved={card => { setSessions(old => ({ ...old, [activeId]: { ...session, cards: [...session.cards, card] } })); setCardDraft(null); notify('已留作复习，可从左下角“复习与历史”找回。'); }} />}
    {toast && <div className="toast" role="status"><Check size={16} /><span>{toast}</span><button className="icon-button" aria-label="关闭提示" onClick={() => setToast('')}><X size={14} /></button></div>}
  </div>;
}

function SourceList({ sources }: { sources: Source[] }) { return <details className="source-list"><summary><FileText size={12} />查看 {sources.length} 个上下文来源 <ChevronDown size={12} /></summary>{sources.map((source, index) => <div className="source-details" key={index}><strong>{source.title}</strong><p>{source.path}</p>{source.excerpt && <blockquote>{source.excerpt}</blockquote>}</div>)}</details>; }
function ExerciseCard({ exercise, onAnswer, onReason }: { exercise: Exercise; onAnswer: (answer: string) => Promise<void>; onReason: (reason: string) => Promise<void> }) {
  const { answer, setAnswer, reason, setReason, error: draftError, retry: retryDraft, conflict: draftConflict, resolveConflict } = useExerciseDraft(exercise.id, exercise.answer || '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [saved, setSaved] = useState(false);
  async function perform(action: () => Promise<void>) { setBusy(true); setError(''); try { await action(); } catch (error) { setError(errorText(error)); } finally { setBusy(false); } }
  return <article className="exercise-card"><span className="card-label">针对练习</span><Markdown text={exercise.prompt} /><label>我的答案<textarea aria-label="练习答案" rows={3} value={answer} onChange={event => setAnswer(event.target.value)} placeholder="先试着用自己的话回答…" /></label><button className="primary-button full-width" disabled={busy || !answer.trim() || (exercise.answer === answer && !!exercise.feedback)} onClick={() => { void perform(() => onAnswer(answer)); }}>{busy ? <LoaderCircle className="spin" size={14} /> : <Check size={14} />}{exercise.answer === answer && exercise.feedback ? '答案已保存' : '提交答案，查看反馈'}</button>{draftError && <div className="inline-error" role="alert">练习草稿尚未备份：{draftError}{draftConflict ? <><p>旧答案：{draftConflict.answer || '空'}</p><p>旧错因：{draftConflict.reason || '空'}</p><div className="button-row"><button onClick={() => resolveConflict(true)}>恢复旧草稿</button><button onClick={() => resolveConflict(false)}>保留当前输入并备份</button></div></> : <button onClick={() => { void retryDraft(); }}>重试读取草稿</button>}</div>}{exercise.feedback && <div className="exercise-feedback"><span className="card-label">{exercise.correct === true ? '理解到位' : exercise.correct === false ? '再看一眼这里' : '学习反馈'}</span><Markdown text={exercise.feedback} /><label>给下次自己的提醒<textarea aria-label="我的错因" rows={2} value={reason} onChange={event => { setReason(event.target.value); setSaved(false); }} placeholder="我为什么会这样理解？下次注意什么？" /></label><button className="quiet-button" disabled={busy || !reason.trim() || saved} onClick={() => { void perform(async () => { await onReason(reason); setSaved(true); }); }}>{saved ? <Check size={13} /> : <Plus size={13} />}{saved ? '错因已保存' : '保存我的错因'}</button></div>}{error && <div className="inline-error" role="alert">{error}</div>}</article>;
}
function Dialog({ title, description, onClose, children }: { title: string; description: string; onClose: () => void; children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => { const previous = document.activeElement as HTMLElement; ref.current?.querySelector<HTMLElement>('input,textarea,button')?.focus(); return () => previous?.focus(); }, []);
  return <div className="dialog-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}><div className="dialog" role="dialog" aria-modal="true" aria-labelledby="dialog-title" ref={ref} onKeyDown={event => { if (event.key !== 'Tab') return; const elements = ref.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),textarea:not(:disabled),select:not(:disabled),[tabindex="0"]'); if (!elements?.length) return; const first = elements[0]; const last = elements[elements.length - 1]; if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); } }}><button className="dialog-close icon-button" aria-label="关闭弹窗" onClick={onClose}><X size={19} /></button><span className="dialog-eyebrow">LEARNING STATION</span><h2 id="dialog-title">{title}</h2><p className="dialog-description">{description}</p>{children}</div></div>;
}
function ImportDialog({ onClose, onImported, draft, onDraftLoaded }: { onClose: () => void; onImported: (section: Section) => void; draft: React.RefObject<{ title: string; body: string }>; onDraftLoaded: (loaded: boolean) => void }) {
  const [title, setTitle] = useState(draft.current.title); const [body, setBody] = useState(draft.current.body); const [busy, setBusy] = useState(false); const [error, setError] = useState(''); const file = useRef<HTMLInputElement>(null);
  const [draftLoaded, setDraftLoaded] = useState(!nativeAvailable);
  const edited = useRef(Boolean(draft.current.title || draft.current.body));
  const loadedRef = useRef(!nativeAvailable);
  const [savedImport, setSavedImport] = useState<UiDraft | null>(null);
  async function restoreDraft() {
    if (!nativeAvailable) return;
    loadedRef.current = false; setDraftLoaded(false); onDraftLoaded(false);
    try { const saved = await call<UiDraft | null>('draft_get', { key: 'workspace' }); if (saved && edited.current && draftInputsDiffer('workspace', { importTitle: draft.current.title, importBody: draft.current.body }, saved.data)) { setSavedImport(saved); setError('发现不同的导入草稿，请先选择保留哪份。'); return; } if (saved && !edited.current) { setTitle(saved.data.importTitle || ''); setBody(saved.data.importBody || ''); } loadedRef.current = true; setDraftLoaded(true); onDraftLoaded(true); setError(''); } catch (error) { setError(errorText(error)); }
  }
  useEffect(() => { void restoreDraft(); return () => { if (nativeAvailable && loadedRef.current && draft.current.body) void call('draft_save', { key: 'workspace', data: { importTitle: draft.current.title, importBody: draft.current.body } }).catch(() => undefined); }; }, []);
  useEffect(() => { draft.current = { title, body }; if (!nativeAvailable || !draftLoaded) return; const timer = setTimeout(() => { void call('draft_save', { key: 'workspace', data: { importTitle: title, importBody: body } }).catch(error => setError(`导入草稿尚未备份：${errorText(error)}`)); }, 600); return () => clearTimeout(timer); }, [title, body, draft, draftLoaded]);
  async function submit(event: React.FormEvent) { event.preventDefault(); if (!body.trim() || busy) return; setBusy(true); setError(''); try { onImported(await call<Section>('section_import', { title: title.trim() || '未命名小节', body })); } catch (error) { setError(errorText(error)); } finally { setBusy(false); } }
  return <Dialog title="打开一份自己的材料" description="一篇短文、一页笔记，或今天要读的小节。系统会自动保存为课程材料。" onClose={onClose}><form onSubmit={event => { void submit(event); }}><input ref={file} type="file" accept=".md,.markdown,.txt,text/plain,text/markdown" hidden onChange={async event => { const selected = event.target.files?.[0]; if (!selected) return; edited.current = true; if (selected.size > 1_500_000) { setError('这份文件超过 1.5MB，请先按小节拆分。'); return; } try { setBody(await selected.text()); if (!title) setTitle(selected.name.replace(/\.(md|markdown|txt)$/i, '')); } catch { setError('无法读取这份文件，请确认文件可用后重试，也可以粘贴正文。'); } }} /><button type="button" className="file-picker" onClick={() => file.current?.click()}><FolderOpen size={20} /><span>选择 Markdown 或文本文件<small>也可以直接粘贴在下方</small></span><Plus size={17} /></button><label>小节名称<input value={title} onChange={event => { edited.current = true; setTitle(event.target.value); }} placeholder="例如：阅读 · Daily habits" maxLength={200} /></label><label>小节正文<textarea className="import-body" value={body} onChange={event => { edited.current = true; setBody(event.target.value); }} placeholder="把自己的学习材料粘贴在这里…" required rows={8} /></label>{error && <div className="inline-error" role="alert">{error}{savedImport ? <><p>旧名称：{savedImport.data.importTitle || '（空）'}</p><details><summary>查看旧正文</summary><Markdown text={savedImport.data.importBody || '（空）'} /></details><div className="button-row"><button type="button" onClick={() => { setTitle(savedImport.data.importTitle || ''); setBody(savedImport.data.importBody || ''); loadedRef.current = true; setDraftLoaded(true); onDraftLoaded(true); setSavedImport(null); setError(''); }}>恢复旧导入草稿</button><button type="button" onClick={() => { loadedRef.current = true; setDraftLoaded(true); onDraftLoaded(true); setSavedImport(null); setError(''); }}>保留当前输入并备份</button></div></> : !draftLoaded && <button type="button" onClick={() => { void restoreDraft(); }}>重试读取导入草稿</button>}</div>}<div className="dialog-footer"><span>正文可随时编辑</span><button className="primary-button" disabled={busy || !body.trim()}>{busy ? <LoaderCircle className="spin" size={15} /> : <ArrowRight size={15} />}{busy ? '正在保存到本机…' : '保存并开始阅读'}</button></div></form></Dialog>;
}
function SettingsDialog({ model, dataPath, onClose, onSaved, font }: { model: ModelSettings; dataPath?: string; onClose: () => void; onSaved: (model: ModelSettings) => void; font: ReturnType<typeof useFontSize> }) {
  const [provider, setProvider] = useState(model.provider === 'glm-pi' ? 'glm-pi' : 'custom');
  const [protocol, setProtocol] = useState(model.protocol || supportedProtocol);
  const [baseUrl, setBaseUrl] = useState(model.baseUrl || '');
  const [name, setName] = useState(model.model || '');
  const [secret, setSecret] = useState('');
  const [busy, setBusy] = useState<'save' | 'test' | ''>('');
  const [error, setError] = useState('');
  const [result, setResult] = useState('');
  const bindingChanged = modelBindingChanged(model, { provider, protocol, baseUrl });
  const settingsChanged = bindingChanged || name.trim() !== model.model;
  async function save(event: React.FormEvent) {
    event.preventDefault(); if (busy) return;
    setError(''); setResult('');
    const validation = modelFormError({ provider, protocol, baseUrl, model: name, apiKey: secret }, model);
    if (validation) { setError(validation); return; }
    setBusy('save'); const apiKey = secret; setSecret('');
    try { const saved = await call<ModelSettings>('model_save', { provider, protocol, baseUrl: baseUrl.trim(), model: name.trim(), apiKey: apiKey || undefined }); onSaved(saved); setBaseUrl(saved.baseUrl); setName(saved.model); setProtocol(saved.protocol || supportedProtocol); setResult(modelAccessMessage(saved) ? `配置已保存到本机。${modelAccessMessage(saved)}` : '配置已保存到本机，尚未发送模型请求。可以继续测试已保存的连接。'); }
    catch (error) { setError(`连接配置尚未保存：${errorText(error)}`); }
    finally { setBusy(''); }
  }
  async function test() {
    if (busy) return; setError(''); setResult('');
    if (!model.configured || settingsChanged) { setError(settingsChanged ? '配置有更改，请先保存，再测试这个目标。' : '请先填写并保存模型连接，再测试。'); return; }
    if (modelAccessMessage(model)) { setError(`本地用途限制：${modelAccessMessage(model)}`); return; }
    setBusy('test');
    try { const response = await call<{ ok: boolean; message: string }>('model_test'); setResult(response.message); onSaved(await call<ModelSettings>('model_get')); }
    catch (error) { setError(modelFailureMessage(error, 'test')); }
    finally { setBusy(''); }
  }
  return <Dialog title="模型与本机设置" description="填写你自己的模型服务。保存只写入本机，点击测试或发送问题时才请求已保存的地址。" onClose={onClose}>
    <section className="font-settings" aria-label="内容字号设置"><div><strong>内容字号</strong><output aria-live="polite">{font.fontSize}px</output></div><div className="font-controls"><button className="quiet-button" aria-label="减小内容字号" disabled={font.fontSize <= 12} onClick={() => font.setFontSize(font.fontSize - 1)}>A−</button><input aria-label="内容字号" type="range" min="12" max="24" step="1" value={font.fontSize} onChange={event => font.setFontSize(Number(event.target.value))} /><button className="quiet-button" aria-label="增大内容字号" disabled={font.fontSize >= 24} onClick={() => font.setFontSize(font.fontSize + 1)}>A＋</button></div><p>阅读、编辑、对话与复习内容同步调整。{font.status}</p>{font.error && <div className="inline-error" role="alert">字号设置：{font.error}<button onClick={() => { void font.flush().catch(() => undefined); }}>重试保存字号</button><button onClick={() => { void font.retry(); }}>重新读取字号</button></div>}</section>
    <div className={`settings-status ${model.connected && !modelAccessReason(model) ? 'connected' : ''}`}><span className="connection-dot" />{modelAccessReason(model) ? '已保存配置 · 本地用途限制' : model.connected ? '已保存目标的真实连接已验证' : model.configured ? '已保存配置 · 连接待验证' : '模型服务尚未配置'}</div>
    {modelAccessMessage(model) && <div className="inline-error model-policy" role="status"><strong>当前保存的地址受本地限制</strong><p>{modelAccessMessage(model)}</p></div>}
    <form noValidate onSubmit={event => { void save(event); }}>
      <label>模型服务类型<select aria-label="模型服务类型" value={provider} onChange={event => { setProvider(event.target.value); setSecret(''); }}><option value="custom">自定义模型服务</option><option value="glm-pi">GLM Pi · Coding Plan 用途受限</option></select></label>
      {provider === 'glm-pi' && !modelAccessReason(model) && <div className="inline-error model-policy" role="status"><strong>Coding Plan 用途限制仍有效</strong><p>Pi 接入不代表套餐授权。此用途尚未确认，服务端会保留请求门槛；不会自动切换到付费 API。自定义服务请使用你获准调用的接口。</p></div>}
      <label>模型协议<select aria-label="模型协议" value={protocol} onChange={event => { setProtocol(event.target.value); setSecret(''); }}><option value={supportedProtocol}>OpenAI Chat Completions</option></select><span className="label-note">当前支持 openai-completions，由 Pi 适配</span></label>
      <label>服务地址 base_url<input aria-label="模型服务地址" type="url" value={baseUrl} onChange={event => { setBaseUrl(event.target.value); setSecret(''); }} placeholder="填写服务提供方给出的基础 base_url" spellCheck={false} autoComplete="off" /><span className="label-note">填写基础地址，不含 /chat/completions；测试也会调用该地址下的服务，并按服务方规则计费。</span></label>
      <label>模型名称<input aria-label="模型名称" value={name} onChange={event => setName(event.target.value)} placeholder="填写服务端支持的模型 ID" spellCheck={false} autoComplete="off" /></label>
      <label>API 密钥 <span className="label-note">{model.credentialStored && !bindingChanged ? '已由本机安全存储管理；留空保留' : '更改地址或协议后需要新密钥'}</span><input aria-label="API 密钥" type="password" value={secret} onChange={event => setSecret(event.target.value)} placeholder={model.credentialStored && !bindingChanged ? '留空保留已有密钥' : '请在本机输入此服务的密钥'} autoComplete="new-password" spellCheck={false} /></label>
      <p className="muted small">密钥不进入网页存储或日志；提交后立即清空输入框。测试只针对已保存的地址、协议和模型。</p>
      {error && <div className="inline-error" role="alert">{error}</div>}{result && <p className="settings-result" role="status">{result}</p>}
      <div className="dialog-footer"><button type="button" className="quiet-button" disabled={!!busy} onClick={() => { void test(); }}>{busy === 'test' ? <><LoaderCircle size={14} className="spin" />正在测试已保存连接…</> : modelAccessReason(model) && !settingsChanged ? '查看本地用途限制' : '测试已保存连接'}</button><button className="primary-button" disabled={!!busy}>{busy === 'save' ? <><LoaderCircle size={14} className="spin" />正在保存配置…</> : '保存连接设置'}</button></div>
    </form>
    <div className="settings-local"><p>飞书提醒：未启用 · Anki：仅 CSV 导出，未同步</p><strong><FolderOpen size={14} />本机材料目录</strong><p>{dataPath || '由本机服务管理，连接后显示路径'}</p><p>Markdown 是学习内容的原始文件，保存时会检查版本变化。</p></div>
  </Dialog>;
}

function CardDialog({ draft, sectionId, onClose, onSaved }: { draft: CardDraft; sectionId: string; onClose: () => void; onSaved: (card: ReviewCard) => void }) {
  const [front, setFront] = useState(draft.front); const [back, setBack] = useState(draft.back); const [busy, setBusy] = useState(false); const [error, setError] = useState('');
  async function submit(event: React.FormEvent) { event.preventDefault(); setBusy(true); setError(''); try { onSaved(await call<ReviewCard>('card_create', { sectionId, front, back })); } catch (error) { setError(errorText(error)); } finally { setBusy(false); } }
  return <Dialog title="留一个问题，给未来的自己" description="原文来源会自动关联。可以只保留选句，也可以写下自己的理解。" onClose={onClose}><form onSubmit={event => { void submit(event); }}><label>下次想回忆什么<textarea value={front} onChange={event => setFront(event.target.value)} rows={4} required /></label><label>我的理解或提醒（可选）<textarea value={back} onChange={event => setBack(event.target.value)} rows={4} placeholder="用自己的话，写下你希望记住的…" /></label>{error && <div className="inline-error" role="alert">{error}</div>}<div className="dialog-footer"><span>之后可导出 CSV</span><button className="primary-button" disabled={busy || !front.trim()}>{busy ? <LoaderCircle className="spin" size={14} /> : <Layers2 size={14} />}保存到复习</button></div></form></Dialog>;
}

function ManualExerciseDialog({ sectionId, onClose, onSaved }: { sectionId: string; onClose: () => void; onSaved: (session: LearningSession) => void }) {
  const exerciseKey = useRef(crypto.randomUUID().replaceAll('-', ''));
  const [prompt, setPrompt] = useState(''); const [reference, setReference] = useState(''); const [exact, setExact] = useState(false); const [busy, setBusy] = useState(false); const [error, setError] = useState('');
  async function submit(event: React.FormEvent) { event.preventDefault(); setBusy(true); setError(''); try { onSaved(await call<LearningSession>('exercise_create_manual', { sectionId, prompt, reference, exact, id: exerciseKey.current })); } catch (error) { setError(errorText(error)); } finally { setBusy(false); } }
  return <Dialog title="添加一道自己的练习" description="不用连接模型。写下题目与参考答案，作答后可以对照反馈并保存错因。" onClose={onClose}><form onSubmit={event => { void submit(event); }}><label>题目<textarea aria-label="手动练习题目" rows={3} value={prompt} onChange={event => setPrompt(event.target.value)} required placeholder="例如：用 instead of 改写这句话…" /></label><label>参考答案或检查要点<textarea aria-label="练习参考答案" rows={3} value={reference} onChange={event => setReference(event.target.value)} required placeholder="写下对照答案，或你想检查的要点…" /></label><label className="checkbox-label"><input type="checkbox" checked={exact} onChange={event => setExact(event.target.checked)} />按唯一短答案比较（忽略大小写与多余空格）</label><p className="muted small">开放回答默认仅展示参考，不自动判定对错。题目与参考由你手动提供。</p>{error && <div className="inline-error" role="alert">{error}</div>}<div className="dialog-footer"><span>来源自动关联当前小节</span><button className="primary-button" disabled={busy || !prompt.trim() || !reference.trim()}>{busy ? <LoaderCircle className="spin" size={14} /> : <Plus size={14} />}保存题目</button></div></form></Dialog>;
}
