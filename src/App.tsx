import { InsertContent } from './EmbeddedContent';
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react';
import { ArrowLeft, ArrowRight, ArrowUp, CornerUpLeft, BookOpen, Check, CheckCheck, ChevronDown, ChevronRight, CircleHelp, Clock3, FileText, FolderOpen, Compass, Layers2, ListTodo, LoaderCircle, MoreHorizontal, Paperclip, PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen, PencilLine, Plus, Sparkles, Square, X } from 'lucide-react';
import { call, emptyBootstrap, isDemo, nativeAvailable, recordMetric, sampleBody } from './api';
const Editor = lazy(() => import('./Editor').then(module => ({ default: module.Editor })));
import { noticeBody } from './noticePolicy';
import { modelAccessReason, modelFailureMessage, modelSelectionError } from './modelPolicy';
import { ExerciseCard } from './ExerciseCard';
import { ReviewPage } from './ReviewPage';
import { addContextInjection, freezeInjections } from './contextInjection';
import { SettingsPage } from './SettingsPage';
import { ChatModelPicker } from './ChatModelPicker';
import { mergeOutgoing, reconcileOutgoing, retryOperation, sendStageLabel, stableRunId } from './outgoingMessages';
import type { OutgoingMessage } from './outgoingMessages';
import { foldToolActivities } from './toolActivities';
import { ToolActivityCards } from './ToolActivityCards';
import { useAssistantWidth } from './useAssistantWidth';
import { useSidebarWidth } from './useSidebarWidth';
import { AssistantResizeHandle } from './AssistantResizeHandle';
import type { ToolActivity } from './types';
import { WorkspaceSidebar } from './WorkspaceSidebar';
import { ExplorationStart } from './ExplorationStart';
import type { ExplorationStartHandle } from './ExplorationStart';
import { TrashConfirmation, TrashPanel } from './TrashPanel';
import { SessionHistory } from './SessionHistory';
import { useNavigationPreferences } from './useNavigationPreferences';
import { ancestorIds, containingCourse } from './treePolicy';
import { isExplorationDocument } from './explorationPolicy';
import { SelectionContextTray, ComposerReferenceChips, composerReferences } from './SelectionContextTray';
import type { ComposerReference } from './SelectionContextTray';
import { MessageReferences } from './MessageReferences';
import { AssistantReplyMenu } from './AssistantQuote';
import { ExerciseQuote } from './ExerciseQuote';
import { addExerciseReference, exerciseReferenceKey, freezeExerciseReferences, validateExerciseSelection } from './exerciseReferences';
import type { AssistantReplyMenuState } from './AssistantQuote';
import { addAssistantReference, assistantTextHash, freezeAssistantReferences, validateAssistantSelection, assistantReplyContent, assistantReplySelection } from './assistantReferences';
import { CreateNodeForm, MoveNodeForm, isContainerKind } from './NodeDialogs';
import type { CreatableKind } from './NodeDialogs';
import { CourseAssets, AssetPreviewContent } from './CourseAssets';
import { LinkedMaterials } from './LinkedMaterials';
import type { EditorHandle } from './Editor';
import { RequestContextView } from './RequestContextView';
import type { ContextViewState } from './RequestContextView';
import { useFontSize } from './useFontSize';
import { Markdown } from './Markdown';
import { composerKeyAction } from './keyboard';
import { draftInputsDiffer, questionForSection, unbackedSectionHasInput } from './draftPolicy';
import { flushExerciseDrafts, hasExerciseInput } from './exerciseDraft';
import { onGenerationProgress } from './native';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { GenerationProgress } from './native';
import type { Bootstrap, Exercise, LibraryNode, LibraryTreeSnapshot, CourseAssetPreview, LearningSession, ModelSettings, ReviewCard, Section, Source, Task, UiDraft, RequestContext, ExplorationNote, TrashReceipt, ContextInjection, AssistantReference, Message, ExerciseReference, ExerciseReferenceSource, ExerciseReferencePart } from './types';
import { StationError } from './types';

type View = 'lesson' | 'review' | 'tasks' | 'settings' | 'explorations' | 'history' | 'history-index' | 'trash';
type Panel = 'explain' | 'practice';
type DocState = { section: Section; body: string; savedBody: string; saving: boolean; error?: string; conflict?: boolean; recovery?: boolean; draftLoaded?: boolean };
type CardDraft = { front: string; back: string; sessionId?: string; sectionId?: string };
const emptySession = (id: string): LearningSession => ({ id: '', sectionId: id, messages: [], exercises: [], mistakes: [], cards: [] });
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);
const makeId = () => crypto.randomUUID();

export function App() {
  const [insertingContent, setInsertingContent] = useState(false);

  const navigation = useNavigationPreferences();
  const navigationRef = useRef(navigation); navigationRef.current = navigation;
  const nodesRef = useRef<LibraryNode[]>([]);
  const editorRefs = useRef(new Map<string, EditorHandle>());
  const [creatingNode, setCreatingNode] = useState<CreatableKind | null>(null);
  const [movingNode, setMovingNode] = useState(false);
  const [explorationEntryId, setExplorationEntryId] = useState(makeId);
  const explorationEntryRef = useRef(explorationEntryId);
  const explorationBackups = useRef(new Set<ExplorationStartHandle>());
  const explorationStart = useRef<ExplorationStartHandle>(null);
  const explorationFolderId = useRef('');
  const treeSequence = useRef(0);
  const [trashNode, setTrashNode] = useState<LibraryNode | null>(null);

  const [linkingMaterials, setLinkingMaterials] = useState(false);
  const linkWrites = useRef(new Set<string>());
  const [assetsOpen, setAssetsOpen] = useState(false);
  const [assetPreview, setAssetPreview] = useState<CourseAssetPreview | null>(null);
  const [assetLoading, setAssetLoading] = useState(false);
  const assetSequence = useRef(0);
  const [practiceFailure, setPracticeFailure] = useState<{ operationId: string; sourceId: string; selection: string; prompt: string; injections: ContextInjection[]; assistantReferences: AssistantReference[]; exerciseReferences: ExerciseReference[]; userEntryId?: string; unknown: boolean; recovery?: boolean } | null>(null);
  const [historySession, setHistorySession] = useState<LearningSession | null>(null);
  const [historyError, setHistoryError] = useState('');
  const font = useFontSize();
  const sidebarWidth = useSidebarWidth();
  const sidebarWidthRef = useRef(sidebarWidth); sidebarWidthRef.current = sidebarWidth;
  const assistantWidth = useAssistantWidth();
  const widthRef = useRef(assistantWidth); widthRef.current = assistantWidth;
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
  const navigationIntent = useRef(0);
  const viewRef = useRef<View>('lesson');
  function switchView(next: View) { const started = performance.now(); navigationIntent.current++; viewRef.current = next; setView(next); setOpening(false); requestAnimationFrame(() => recordMetric('page-switch', performance.now() - started)); }
  useEffect(() => { const frame = requestAnimationFrame(() => recordMetric('shell-interactive', performance.now())); return () => cancelAnimationFrame(frame); }, []);
  const [editing, setEditing] = useState(false);
  const [editedIds, setEditedIds] = useState<Set<string>>(new Set());
  const [contextView, setContextView] = useState<ContextViewState>({ status: 'idle' });
  const [contextRetry, setContextRetry] = useState(0);
  const contextSequence = useRef(0);
  const contextInputs = useRef({ sessionId: '', documentId: '', body: '', version: '', selection: '', injections: '' });
  const actualContextInputs = useRef<typeof contextInputs.current | null>(null);
  const [rightbar, setRightbar] = useState(true);
  const [selection, setSelectionState] = useState('');
  const selectionStateRef = useRef({ value: '', revision: 0 });
  function setSelection(value: string) { selectionStateRef.current = { value, revision: selectionStateRef.current.revision + 1 }; setSelectionState(value); }
  const [injectionsBySession,setInjectionsBySession]=useState<Record<string,ContextInjection[]>>({});
  const injectionsRef=useRef(injectionsBySession);injectionsRef.current=injectionsBySession;
  const [assistantReferencesBySession, setAssistantReferencesBySession] = useState<Record<string, AssistantReference[]>>({});
  const assistantReferencesRef = useRef(assistantReferencesBySession); assistantReferencesRef.current = assistantReferencesBySession;
  const [exerciseReferencesBySession, setExerciseReferencesBySession] = useState<Record<string, ExerciseReference[]>>({});
  const exerciseReferencesRef = useRef(exerciseReferencesBySession); exerciseReferencesRef.current = exerciseReferencesBySession;
  const [exerciseQuote, setExerciseQuote] = useState<ExerciseReferenceSource | null>(null);
  const exerciseQuoteSequence = useRef(0);
  const [assistantReplyMenu, setAssistantReplyMenu] = useState<AssistantReplyMenuState | null>(null);
  const assistantQuoteSequence = useRef(0);
  const referenceRevisions = useRef(new WeakMap<object, number>());
  const referenceSequence = useRef(0);
  const referenceConsumption = useRef(new Map<string, { sessionId: string; documentId: string; selection: string; selectionRevision: number; injections: Map<string, number>; answers: Map<string, number>; exercises: Map<string, number> }>());
  function referenceRevision(item: object) { let revision = referenceRevisions.current.get(item); if (revision === undefined) { revision = ++referenceSequence.current; referenceRevisions.current.set(item, revision); } return revision; }
  function rememberReferenceConsumption(id: string, sessionId: string, documentId: string, selection: string) {
    referenceConsumption.current.set(id, { sessionId, documentId, selection, selectionRevision: selectionStateRef.current.revision, injections: new Map((injectionsRef.current[sessionId] || []).map(item => [item.documentId, referenceRevision(item)])), answers: new Map((assistantReferencesRef.current[sessionId] || []).map(item => [item.entryId, referenceRevision(item)])), exercises: new Map((exerciseReferencesRef.current[sessionId] || []).map(item => [exerciseReferenceKey(item), referenceRevision(item)])) });
  }
  function consumeReferenceSnapshot(id: string) {
    const snapshot = referenceConsumption.current.get(id); if (!snapshot) return;
    referenceConsumption.current.delete(id);
    const current = injectionsRef.current[snapshot.sessionId] || [];
    const kept = current.filter(item => snapshot.injections.get(item.documentId) !== referenceRevision(item));
    if (kept.length !== current.length) { const next = { ...injectionsRef.current, [snapshot.sessionId]: kept }; injectionsRef.current = next; setInjectionsBySession(next); }
    const answers = assistantReferencesRef.current[snapshot.sessionId] || [];
    const keptAnswers = answers.filter(item => snapshot.answers.get(item.entryId) !== referenceRevision(item));
    if (keptAnswers.length !== answers.length) { const next = { ...assistantReferencesRef.current, [snapshot.sessionId]: keptAnswers }; assistantReferencesRef.current = next; setAssistantReferencesBySession(next); }
    const exercises = exerciseReferencesRef.current[snapshot.sessionId] || [];
    const keptExercises = exercises.filter(item => snapshot.exercises.get(exerciseReferenceKey(item)) !== referenceRevision(item));
    if (keptExercises.length !== exercises.length) { const next = { ...exerciseReferencesRef.current, [snapshot.sessionId]: keptExercises }; exerciseReferencesRef.current = next; setExerciseReferencesBySession(next); }
    if (snapshot.selection && activeIdRef.current === snapshot.documentId && selectionStateRef.current.revision === snapshot.selectionRevision && selectionStateRef.current.value === snapshot.selection) setSelection('');
  }
  const [injectionMenu,setInjectionMenu]=useState<{id:string;selection?:string;x:number;y:number;exercise?:{sessionId:string;questionId:string;part:ExerciseReferencePart;entryId?:string}}|null>(null);
  const [question, setQuestion] = useState('');
  const [localQuestionSaving, setLocalQuestionSaving] = useState(false);
  const [busyId, setBusyId] = useState('');
  const [partial, setPartial] = useState<GenerationProgress | null>(null);
  const [outgoing, setOutgoing] = useState<OutgoingMessage[]>([]);
  const outgoingRef = useRef<OutgoingMessage[]>([]);
  function updateOutgoing(change: (items: OutgoingMessage[]) => OutgoingMessage[]) { const next = change(outgoingRef.current); outgoingRef.current = next; setOutgoing(next); }
  function patchOutgoing(sendId: string, patch: Partial<OutgoingMessage>) { updateOutgoing(items => items.map(item => item.sendId === sendId ? { ...item, ...patch } : item)); }
  const confirmedPracticeUsers = useRef(new Map<string, string>());
  const [liveTools, setLiveTools] = useState<Record<string, ToolActivity[]>>({});
  const [activeRunId, setActiveRunId] = useState('');
  const [objectPreview, setObjectPreview] = useState<{ kind: string; id: string; title: string; body?: string; front?: string; back?: string; dueAt?: string; done?: boolean; reminderEnabled?: boolean } | null>(null);
  useEffect(() => onGenerationProgress(progress => { if (progress.operationId !== busyRef.current) return; if (progress.userEntryConfirmed) { if (progress.mode === 'practice' && progress.userEntryId) confirmedPracticeUsers.current.set(progress.operationId, progress.userEntryId); consumeReferenceSnapshot(progress.operationId); } if (progress.content) setPartial(progress); if (progress.runId) setActiveRunId(progress.runId); if (progress.toolCalls) setLiveTools(old => ({ ...old, [progress.sessionId]: foldToolActivities([...(old[progress.sessionId] || []), ...progress.toolCalls!], progress.sessionId) })); if (progress.context) { if (contextInputs.current.sessionId === progress.sessionId) actualContextInputs.current = contextInputs.current; setContextView({ status: 'ready', actual: true, sessionId: progress.sessionId, context: progress.context }); } else if (progress.manifest) setContextView({ status: 'ready', actual: true, sessionId: progress.sessionId, context: { text: '', manifest: progress.manifest, warnings: [], truncated: false } }); if (progress.contextError) setContextView({ status: 'error', sessionId: progress.sessionId, error: progress.contextError }); }), []);
  const busyRef = useRef('');
  const [aiError, setAiError] = useState('');
  const [importing, setImporting] = useState(false);
  const materialDraft = useRef({ title: '', body: '' });
  const settingsReturnView = useRef<View>('lesson');
  function openSettings() { if (view !== 'settings') settingsReturnView.current = view; switchView('settings'); }
  const [modelChanging, setModelChanging] = useState(false);
  const modelChangingRef = useRef(false);
  async function changeModel(name: string) {
    if (busyRef.current || modelChangingRef.current) throw new StationError('请先停止生成，再更换模型。', 'BUSY');
    const validation = modelSelectionError(name, boot.model);
    if (validation) throw new StationError(validation, 'INVALID_MODEL_SETTINGS');
    modelChangingRef.current = true; setModelChanging(true);
    try { const model = await call<ModelSettings>('model_select', { model: name.trim() }); setBoot(old => ({ ...old, model })); }
    finally { modelChangingRef.current = false; setModelChanging(false); }
  }
  const [manualExercise, setManualExercise] = useState(false);
  const [backupConflict, setBackupConflict] = useState<UiDraft | null>(null);
  const workspaceDraftLoaded = useRef(false);
  const [comparison, setComparison] = useState<Section | null>(null);
  const [cardDraft, setCardDraft] = useState<CardDraft | null>(null);
  const [toast, setToast] = useState('');
  useEffect(() => {
    if (!nativeAvailable) return;
    const subscription = listen<{ id: string; title: string; read: boolean; deliveryStatus?: string }>('notification', event => { setBoot(old => ({ ...old, notices: [...old.notices.filter(item => item.id !== event.payload.id), { ...event.payload, body: noticeBody(event.payload.deliveryStatus) }] })); });
    return () => { void subscription.then(unlisten => unlisten()); };
  }, []);
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
  const session = sessions[activeId] ?? emptySession(activeId);
  const nodes: LibraryNode[] = boot.nodes || boot.courses.flatMap(course => course.sections.map(item => ({ ...item, kind: 'section' as const, parentId: null, path: '', version: '' }))); nodesRef.current = nodes;
  const isContainer = isContainerKind(current?.section.kind);
  const courseId = containingCourse(nodes, activeId)?.id || nodes.find(node => node.id === activeId)?.courseId || current?.section.courseId || activeId;
  const conversation = mergeOutgoing(session.messages, outgoing, session.id);
  const toolCalls = foldToolActivities([...(session.toolCalls || []), ...(liveTools[session.id] || [])], session.id);
  const displayedMessages = conversation.slice(-30);
  const repliedRuns = new Set(displayedMessages.filter(message=>message.role==='assistant'&&message.runId).map(message=>message.runId));
  const composerQuestion = question;
  const requestSelection = session.sectionId && current?.section.kind !== 'exercise' ? selection : '';
  const injections=injectionsBySession[session.id]||[];
  const assistantReferences = assistantReferencesBySession[session.id] || [];
  const exerciseReferences = exerciseReferencesBySession[session.id] || [];
  const injectionKey=JSON.stringify([injections,assistantReferences,exerciseReferences]);
  const composerRefs = composerReferences(requestSelection, session.sectionId, current?.section.version || '', current?.section.title || '当前材料', injections, id => nodes.find(node => node.id === id)?.title || id, assistantReferences, exerciseReferences);
  contextInputs.current = { sessionId: session.id, documentId: activeId, body: current?.body || '', version: current?.section.version || '', selection: requestSelection, injections: injectionKey };
  const canCompose = !!current && !isContainer;
  const visibleSessionRef = useRef(session.id); visibleSessionRef.current = session.id;
  const modelBlocked = !!modelAccessReason(boot.model);
  const modelReady = boot.model.configured && !modelBlocked;
  const dirtyCount = Object.values(docs).filter(doc => doc.body !== doc.savedBody).length;
  const activeCourse = containingCourse(nodes, activeId) || boot.courses.find(course => course.sections.some(section => section.id === activeId));

  const notify = useCallback((message: string) => setToast(message), []);
  useEffect(() => { if (activeId) questionDrafts.current[activeId] = question; }, [activeId, question]);
  useEffect(() => { if (toast) { const timer = setTimeout(() => setToast(''), 4500); return () => clearTimeout(timer); } }, [toast]);
  const openSection = useCallback(async (id: string) => {
    const started = performance.now();
    const sequence = ++openSequence.current;
    navigationRef.current.ensureExpanded(ancestorIds(nodesRef.current, id));
    activeIdRef.current = id; setActiveId(id); setAssetsOpen(false); setQuestion(questionForSection(questionDrafts.current, id)); switchView('lesson'); setEditing(false); setSelection(''); setAiError(''); setOpenError('');
    const intent = navigationIntent.current;
    const stillCurrent = () => intent === navigationIntent.current && sequence === openSequence.current;
    if (docsRef.current[id]) {
      if (!isContainerKind(docsRef.current[id].section.kind)) void call<LearningSession>('session_open', { sectionId: id }).then(value => setSessions(old => ({ ...old, [id]: value }))).catch(error => { if (stillCurrent()) setAiError(`学习记录尚未读到：${errorText(error)}`); });
      if (nativeAvailable) void call<Section>('section_open', { id }).then(section => {
        const local = docsRef.current[id];
        if (!local || local.saving || section.version === local.section.version) return;
        if (local.body !== local.savedBody) { setDocs(old => ({ ...old, [id]: { ...old[id], conflict: true, error: '原文件已在外部更新。当前编辑保留，请先对照新版本。' } })); return; }
        setDocs(old => ({ ...old, [id]: { section, body: section.body, savedBody: section.body, saving: false, draftLoaded: local.draftLoaded } }));
        setEditedIds(old => { const next = new Set(old); next.delete(id); return next; });
      }).catch(error => { if (stillCurrent()) setOpenError(errorText(error)); });
      return;
    }
    setOpening(true);
    try {
      const [section, draft] = await Promise.all([call<Section>('section_open', { id }), nativeAvailable ? call<UiDraft | null>('draft_get', { key: `section:${id}` }).then(value => ({ loaded: true, value })).catch(error => { reportDraftError(`section:${id}`, error); return { loaded: false, value: null }; }) : Promise.resolve({ loaded: true, value: null })]);
      const opened: DocState = { section, body: draft.value?.data.body ?? section.body, savedBody: section.body, saving: false, recovery: draft.value?.data.body !== undefined && draft.value.data.body !== section.body, draftLoaded: draft.loaded };
      // Immediate follow-up actions use the same committed document before React paints.
      docsRef.current = { ...docsRef.current, [id]: opened };
      setDocs(previous => ({ ...previous, [id]: opened }));
      if (stillCurrent() && !isContainerKind(section.kind)) void call<LearningSession>('session_open', { sectionId: id }).then(learning => setSessions(previous => ({ ...previous, [id]: learning }))).catch(error => { if (activeIdRef.current === id) setAiError(`小节已打开，学习记录暂未载入：${errorText(error)}`); });
      requestAnimationFrame(() => recordMetric('section-ready', performance.now() - started));
      if (draft.value?.data.question) { questionDrafts.current[id] = draft.value.data.question; if (stillCurrent()) setQuestion(draft.value.data.question); }
    } catch (error) { if (stillCurrent()) setOpenError(errorText(error)); }
    finally { if (stillCurrent()) setOpening(false); }
  }, []);
  const load = useCallback(async () => {
    const intent = navigationIntent.current;
    const treeRequest = ++treeSequence.current;
    setLoading(true); setBootError('');
    try {
      const result = await call<Bootstrap>('bootstrap');
      setBoot(result);
      // Wait for the real ancestry before restoring: new exploration notes are sections.
      const [tree, folderRunId] = await Promise.all([
        !isDemo ? call<LibraryTreeSnapshot>('library_tree') : Promise.resolve({ nodes: result.nodes || result.courses.flatMap(course => course.sections.map(section => ({ ...section, kind: 'section' as const, parentId: null, path: '', version: '' }))), warnings: [] }),
        stableRunId('learning-station-exploration-folder-v1'),
      ]);
      explorationFolderId.current = folderRunId.replace(/^run_/, 'node_');
      if (treeRequest !== treeSequence.current) return;
      nodesRef.current = tree.nodes;
      setBoot(old => ({ ...old, nodes: tree.nodes, treeWarnings: tree.warnings }));
      if (intent !== navigationIntent.current || intent !== 0) return;
      const id = result.lastSectionId || result.courses[0]?.sections[0]?.id;
      if (id && isExplorationDocument(tree.nodes, id, explorationFolderId.current)) {
        switchView('explorations');
      } else if (id && tree.nodes.some(node => node.id === id)) {
        void openSection(id);
      }
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
    if (!doc || doc.saving || linkWrites.current.has(id) || doc.body === doc.savedBody || doc.conflict) return;
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
      if (outgoingRef.current.some(item => !item.userEntryId)) { event.preventDefault(); notify('有问题尚未写入学习记录，请先在对应会话重试保存。原文仍保留在消息中。'); return; }
      const drafts = Object.entries(docsRef.current).filter(([, doc]) => doc.body !== doc.savedBody);
      const hasInput = drafts.length || Object.values(questionDrafts.current).some(Boolean) || materialDraft.current.body || materialDraft.current.title || hasExerciseInput();
      try { await sidebarWidthRef.current.flush(); } catch (error) { event.preventDefault(); notify(`目录宽度尚未保存，窗口已保留：${errorText(error)}`); return; }
      try { await widthRef.current.flush(); } catch (error) { event.preventDefault(); notify(`面板宽度尚未保存，窗口已保留：${errorText(error)}`); return; }
      try { await fontRef.current.flush(); } catch (error) { event.preventDefault(); notify(`字号尚未保存，窗口已保留：${errorText(error)}`); return; }
      try { await navigationRef.current.flush(); } catch (error) { event.preventDefault(); notify(`导航状态尚未保存，窗口已保留：${errorText(error)}`); return; }
      try { await Promise.all([...explorationBackups.current, ...(explorationStart.current ? [explorationStart.current] : [])].map(draft => draft.flush())); } catch (error) { event.preventDefault(); notify(`探索问题尚未备份，窗口已保留：${errorText(error)}`); return; }
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
      if (event.key === 'Escape') { setInjectionMenu(null); setSelection(''); setImporting(false); setCardDraft(null); }
    };
    window.addEventListener('keydown', key);
    return () => window.removeEventListener('keydown', key);
  }, [activeId, save]);
  useEffect(() => { const area = chatEnd.current?.parentElement; if (area) area.scrollTop = area.scrollHeight; }, [conversation.length, busyId]);

  useEffect(() => {
    const sequence = ++contextSequence.current;
    if (!session.id) { setContextView({ status: 'idle' }); return; }
    if (busyId) { setContextView(old => old.sessionId === session.id && old.actual ? old : { status: 'loading', sessionId: session.id }); return; }
    const actual = actualContextInputs.current; const inputs = contextInputs.current;
    if (contextView.actual && contextView.sessionId === session.id && actual && Object.keys(inputs).every(key => actual[key as keyof typeof actual] === inputs[key as keyof typeof inputs])) return;
    if (current && current.body !== current.savedBody) { setContextView({ status: 'error', sessionId: session.id, error: '正文有未保存改动，保存后会重新核对来源。' }); return; }
    setContextView({ status: 'loading', sessionId: session.id });
    const timer = setTimeout(() => { void call<RequestContext>('context_load', { sessionId: session.id, sectionId: session.sectionId, selection: requestSelection, injections: freezeInjections(injections), assistantReferences: freezeAssistantReferences(assistantReferences), exerciseReferences: freezeExerciseReferences(exerciseReferences) }).then(context => { if (sequence === contextSequence.current) setContextView({ status: 'ready', sessionId: session.id, context }); }).catch(error => { if (sequence === contextSequence.current) setContextView({ status: 'error', sessionId: session.id, error: errorText(error) }); }); }, 150);
    return () => clearTimeout(timer);
  }, [session.id, requestSelection, current?.body, current?.savedBody, current?.section.version, busyId, contextRetry, injectionKey]);
  function updateComposer(value: string) { setQuestion(value); }
  function updateTargetSession(result: LearningSession, _independentTarget = false) {
    const reconciled = reconcileOutgoing(outgoingRef.current, result.messages, result.id);
    updateOutgoing(() => reconciled);
    for (const item of reconciled) if (item.sessionId === result.id && item.userEntryId && result.messages.some(message => message.role === 'user' && message.id === item.userEntryId)) consumeReferenceSnapshot(item.sendId);
    setSessions(old => ({ ...Object.fromEntries(Object.entries(old).map(([key,value]) => [key, value.id === result.id ? result : value])), [result.sectionId]: result }));
  }
  async function refreshAfterGeneration(target: LearningSession, independentTarget: boolean) {
    const knownNodes = new Set(nodesRef.current.map(node=>node.id));
    const results = await Promise.allSettled([call<LearningSession>('session_refresh', { sectionId: target.sectionId, sessionId: target.id }), call<Pick<Bootstrap, 'tasks' | 'notices'>>('activity_refresh'), refreshTree()]);
    if (results[0].status === 'fulfilled') updateTargetSession(results[0].value, independentTarget); else notify(`学习记录刷新失败，已有内容仍保留：${errorText(results[0].reason)}`);
    if (results[2].status === 'fulfilled' && results[2].value) { const tree = results[2].value; navigation.ensureExpanded(tree.nodes.filter(node=>node.kind==='exercise'&&!knownNodes.has(node.id)).flatMap(node=>ancestorIds(tree.nodes,node.id))); }
    if (results[1].status === 'fulfilled') { const activity = results[1].value; setBoot(old => ({ ...old, ...activity })); } else notify(`待办与通知尚未刷新：${errorText(results[1].reason)}`);
  }
  function removeInjection(id:string){const next={...injectionsRef.current,[session.id]:(injectionsRef.current[session.id]||[]).filter(item=>item.documentId!==id)};injectionsRef.current=next;setInjectionsBySession(next);}
  function removeComposerReference(item: ComposerReference) { if (item.kind === 'exercise') { const next = { ...exerciseReferencesRef.current, [session.id]: (exerciseReferencesRef.current[session.id] || []).filter(value => exerciseReferenceKey(value) !== exerciseReferenceKey(item)) }; exerciseReferencesRef.current = next; setExerciseReferencesBySession(next); } else if (item.kind === 'assistant') { const next = { ...assistantReferencesRef.current, [session.id]: (assistantReferencesRef.current[session.id] || []).filter(value => value.entryId !== item.entryId) }; assistantReferencesRef.current = next; setAssistantReferencesBySession(next); } else if (item.currentSelection) setSelection(''); else removeInjection(item.documentId); }
  async function injectDocument(id:string,selected?:string){
    const selectedRevision = selectionStateRef.current.revision;
    const targetSession=session.id;if(!targetSession){notify('请先打开学习会话。');return;}
    try{const local=docsRef.current[id];if(local&&local.body!==local.savedBody)await prepareSource(id);const document=await call<Section>('section_open',{id});if(!['section','knowledge','exploration'].includes(document.kind||'section'))throw new Error('只可加入学习材料，不能加入目录、练习参考或系统记录。');if(selected&&!document.body.includes(selected))throw new Error('选区与已保存原文不一致，请在原文中重新选择。');const item:ContextInjection={documentId:id,expectedVersion:document.version,...(selected?{selection:selected}:{})};const next={...injectionsRef.current,[targetSession]:addContextInjection(injectionsRef.current[targetSession]||[],item,(assistantReferencesRef.current[targetSession]||[]).length + (exerciseReferencesRef.current[targetSession]||[]).length)};injectionsRef.current=next;setInjectionsBySession(next);if(selected&&activeIdRef.current===id&&visibleSessionRef.current===targetSession&&selectionStateRef.current.revision===selectedRevision&&selectionStateRef.current.value===selected)setSelection('');notify('已显式加入本次上下文，尚未发送模型请求。');}catch(error){notify(errorText(error));}
  }
  function addExerciseQuote(source: ExerciseReferenceSource, selected: string) {
    if (visibleSessionRef.current !== source.sessionId || activeIdRef.current !== source.documentId) { notify('已切换学习文档，未将旧选段加入新页面。'); return; }
    try { validateExerciseSelection(source.text, selected); const item: ExerciseReference = { sessionId: source.sessionId, documentId: source.documentId, exerciseEntryId: source.exerciseEntryId, questionId: source.questionId, part: source.part, entryId: source.entryId, textHash: source.textHash, selection: selected }; const next = { ...exerciseReferencesRef.current, [source.sessionId]: addExerciseReference(exerciseReferencesRef.current[source.sessionId] || [], item, (injectionsRef.current[source.sessionId] || []).length + (assistantReferencesRef.current[source.sessionId] || []).length) }; exerciseReferencesRef.current = next; setExerciseReferencesBySession(next); setExerciseQuote(null); setRightbar(true); }
    catch (error) { notify(errorText(error)); }
  }
  async function quoteExercise(documentId: string, sessionId: string, questionId: string, part: ExerciseReferencePart, entryId: string | undefined, selected: string, position?: {x:number;y:number}) {
    if (position) { setInjectionMenu({ id: documentId, selection: selected, x: Math.min(position.x, window.innerWidth - 230), y: Math.min(position.y, window.innerHeight - 130), exercise: { sessionId, questionId, part, entryId } }); return; }
    const sequence = ++exerciseQuoteSequence.current;
    try { const source = await call<ExerciseReferenceSource>('get_exercise_reference', { sessionId, documentId, questionId, part, ...(entryId ? {entryId} : {}) });
      if (sequence !== exerciseQuoteSequence.current || visibleSessionRef.current !== sessionId || activeIdRef.current !== documentId) return;
      if (source.text.includes(selected)) addExerciseQuote(source, selected); else setExerciseQuote(source);
    } catch (error) { if (sequence === exerciseQuoteSequence.current) notify(`练习选段未加入：${errorText(error)}`); }
  }
  async function replyToAssistant(message: Message, sessionId: string, selected?: string) {
    const sequence = ++assistantQuoteSequence.current, intent = navigationIntent.current;
    try {
      if (visibleSessionRef.current !== sessionId || !canCompose) throw new Error('请先打开这条回答所属的学习会话，再回复。');
      const selection = selected === undefined ? assistantReplyContent(message.content) : selected;
      validateAssistantSelection(message.content, selection);
      const expectedHash = await assistantTextHash(message.content);
      const source = await call<LearningSession>('history_open', { sessionId });
      const stored = source.messages.find(item=>item.id===message.id&&item.role==='assistant');
      if (!stored) throw new Error('这条回答已不存在，未替换成其他内容。');
      if (await assistantTextHash(stored.content) !== expectedHash || stored.runId !== message.runId) throw new Error('回答原文或运行记录已变化，请重新选择；原引用和输入均保留。');
      if (sequence !== assistantQuoteSequence.current || intent !== navigationIntent.current || visibleSessionRef.current !== sessionId) return;
      const item: AssistantReference = { sessionId, entryId: stored.id, ...(stored.runId?{runId:stored.runId}:{}), textHash: expectedHash, selection };
      const next = { ...assistantReferencesRef.current, [sessionId]: addAssistantReference(assistantReferencesRef.current[sessionId] || [], item, (injectionsRef.current[sessionId] || []).length + (exerciseReferencesRef.current[sessionId] || []).length) };
      assistantReferencesRef.current = next; setAssistantReferencesBySession(next); setRightbar(true); switchView('lesson'); requestAnimationFrame(()=>questionRef.current?.focus());
    } catch (error) { if (sequence === assistantQuoteSequence.current) notify(errorText(error)); }
  }
  function assistantMenu(event: React.MouseEvent<HTMLDivElement> | React.KeyboardEvent<HTMLDivElement>, message: Message, sessionId: string) {
    if ('key' in event && (event.nativeEvent.isComposing || event.key !== 'ContextMenu' && !(event.shiftKey && event.key === 'F10'))) return;
    event.preventDefault(); event.stopPropagation();
    try { const body = event.currentTarget; const selected = assistantReplySelection(body, message.content, window.getSelection()); const rect = body.getBoundingClientRect(); setAssistantReplyMenu({message,sessionId,selection:selected,trigger:body,x:'clientX' in event?event.clientX:rect.left+20,y:'clientY' in event?event.clientY:rect.bottom}); }
    catch(error){notify(errorText(error));}
  }
  function closeAssistantMenu(restoreFocus = false) { const trigger=assistantReplyMenu?.trigger; setAssistantReplyMenu(null); if(restoreFocus)requestAnimationFrame(()=>trigger?.focus()); }
  async function openAssistantQuote(item: AssistantReference) {
    const sequence = ++assistantQuoteSequence.current, intent = navigationIntent.current;
    try { const source = await call<LearningSession>('history_open',{sessionId:item.sessionId}); const message=source.messages.find(value=>value.id===item.entryId&&value.role==='assistant');
      if(!message)throw new Error('这条回答已不存在，原引用仍保留。');
      if(await assistantTextHash(message.content)!==item.textHash||message.runId!==item.runId)throw new Error('回答原文或运行记录已变化，原引用和输入仍保留。');
      if(sequence!==assistantQuoteSequence.current||intent!==navigationIntent.current)return;
      const locate=()=>{const target=Array.from(document.querySelectorAll<HTMLElement>('[data-assistant-entry]')).find(element=>element.dataset.assistantEntry===item.entryId&&element.dataset.assistantSession===item.sessionId&&!element.closest('[hidden]'));if(target){target.scrollIntoView?.({block:'center',behavior:'auto'});target.focus();target.classList.add('reply-source-highlight');setTimeout(()=>target.classList.remove('reply-source-highlight'),1300);}return !!target;};
      if(!locate()){setHistorySession(source);setHistoryError('');switchView('history');requestAnimationFrame(locate);}
    } catch(error){if(sequence===assistantQuoteSequence.current&&intent===navigationIntent.current)notify(errorText(error));}
  }
  async function saveLocalQuestion() {
    const value = composerQuestion.trim(); if (!value || localQuestionSaving) return;
    if (!canCompose || !session.id) { setAiError('学习会话尚未就绪，请稍候或重试打开。'); return; }
    const target = session; const documentId = activeId; const detached = false; setLocalQuestionSaving(true);
    try { const result = await call<LearningSession>('question_save', { sectionId: target.sectionId, sessionId: target.id, question: value, selection: requestSelection }); updateTargetSession(result, detached); if (activeIdRef.current === documentId) setQuestion(old => old.trim() === value ? '' : old); notify('困惑已保存在学习记录中，未请求AI。'); }
    catch (error) { setAiError(`困惑尚未保存，输入仍保留：${errorText(error)}`); }
    finally { setLocalQuestionSaving(false); }
  }
  async function executeSend(item: OutgoingMessage, operationId: string) {
    const target = { id: item.sessionId, sectionId: item.sectionId } as LearningSession;
    let userEntryId = item.userEntryId;
    try {
      if (!userEntryId) {
        const saved = await call<{ session: LearningSession; userEntryId: string }>('message_save', { sectionId: item.sectionId, sessionId: item.sessionId, question: item.content, selection: item.selection, mode: item.mode, sendId: item.sendId, injections: item.injections || [], assistantReferences: item.assistantReferences || [], exerciseReferences: item.exerciseReferences || [] });
        userEntryId = saved.userEntryId; patchOutgoing(item.sendId, { userEntryId }); updateTargetSession(saved.session);
      }
      patchOutgoing(item.sendId, { stage: 'preparing' });
      await prepareSource(item.documentId || item.sectionId);
      const context = await call<RequestContext>('context_load', { sessionId: item.sessionId, sectionId: item.sectionId, selection: item.selection, injections: item.injections || [], assistantReferences: item.assistantReferences || [], exerciseReferences: item.exerciseReferences || [] });
      if (visibleSessionRef.current === item.sessionId) setContextView({ status: 'loading', sessionId: item.sessionId, context });
      patchOutgoing(item.sendId, { stage: 'generating' });
      const result = await call<LearningSession>('session_ask', { sectionId: item.sectionId, sessionId: item.sessionId, mode: item.mode, question: item.content, selection: item.selection, userEntryId, operationId, injections: item.injections || [], assistantReferences: item.assistantReferences || [], exerciseReferences: item.exerciseReferences || [], resume: item.failureStage === 'unknown' });
      updateTargetSession(result, item.detached); patchOutgoing(item.sendId, { stage: 'done', error: undefined, failureStage: undefined });
    } catch (error) {
      const details = error instanceof StationError ? error.details as { sendStage?: string } | undefined : undefined;
      patchOutgoing(item.sendId, { stage: 'failed', userEntryId, error: modelFailureMessage(error, 'generation'), failureStage: !userEntryId ? 'save' : details?.sendStage === 'unknown' ? 'unknown' : 'generation' });
    } finally {
      await refreshAfterGeneration(target, item.detached);
      if (busyRef.current === operationId) { busyRef.current = ''; setBusyId(''); }
    }
  }
  async function ask(preset?: string) {
    const value = (preset ?? composerQuestion).trim();
    if (!value || busyRef.current) return;
    if (modelChangingRef.current) { notify('正在更换模型，请稍候再提问。'); return; }
    if (!canCompose || !session.id) { setAiError('学习会话尚未就绪，请重试打开，问题仍保留。'); return; }
    if (!modelReady) { notify(modelBlocked ? '此模型用途受限，请查看供应商设置。问题仍保留，未发送请求。' : '请先保存供应商连接，问题仍保留。'); openSettings(); return; }
    const operationId = makeId();
    const item: OutgoingMessage = { sendId: makeId(), operationId, sessionId: session.id, sectionId: session.sectionId, detached: false, documentId: activeId, mode: isExplorationDocument(nodesRef.current, activeId, explorationFolderId.current) ? 'explore' : 'explain', content: value, selection: requestSelection, injections: freezeInjections(injectionsRef.current[session.id] || []), assistantReferences: freezeAssistantReferences(assistantReferencesRef.current[session.id] || []), exerciseReferences: freezeExerciseReferences(exerciseReferencesRef.current[session.id] || []), createdAt: new Date().toISOString(), stage: 'saving' };
    busyRef.current = operationId; setBusyId(operationId); setPartial(null); setActiveRunId(''); setAiError('');
    rememberReferenceConsumption(item.sendId, item.sessionId, activeId, item.selection);
    updateOutgoing(items => [...items, item]);
    setQuestion(old => old.trim() === value ? '' : old);
    await executeSend(item, operationId);
  }
  async function retrySend(sendId: string) {
    const item = outgoingRef.current.find(value => value.sendId === sendId);
    if (!item || busyRef.current || modelChangingRef.current) return;
    if (!modelReady) { notify('请先检查供应商配置；原消息与新草稿均保留。'); openSettings(); return; }
    const operationId = retryOperation(item, makeId());
    busyRef.current = operationId; setBusyId(operationId); setPartial(null); setActiveRunId('');
    patchOutgoing(sendId, { operationId, stage: item.userEntryId ? 'generating' : 'saving', error: undefined });
    await executeSend(item, operationId);
  }
  async function openToolObject(object: NonNullable<ToolActivity['object']>) {
    try { if(object.kind==='exercise'){await openSection(object.id);return;} setObjectPreview(await call('tool_object_open', object)); }
    catch (error) { notify(`未能打开已保存对象：${errorText(error)}`); }
  }

  async function cancel() {
    const id = busyRef.current; if (!id) return;
    try { await call('operation_cancel', { operationId: id }); notify('已请求停止生成，已保存的对话仍保留。'); }
    catch (error) { notify(`停止请求未确认：${errorText(error)}`); }
  }
  async function generateExercise(retry = false) {
    if (!current || isContainer || current.section.kind === 'exercise' || busyRef.current) return;
    if (modelChangingRef.current) { notify('正在更换模型，请稍候再生成练习。'); return; }
    if (!modelReady) { notify(modelBlocked ? '该模型用途受限，请查看设置说明。' : '请先保存模型连接配置。'); openSettings(); return; }
    if (!retry && practiceFailure?.sourceId === activeId && practiceFailure.unknown) { notify('上次练习结果未确认，请先核对原请求。'); return; }
    const previous = retry && practiceFailure?.sourceId === activeId ? practiceFailure : null;
    const id = previous?.unknown ? previous.operationId : makeId(); const selected = previous ? previous.selection : requestSelection;
    const prompt=previous?previous.prompt:composerQuestion.trim(); const frozen=previous?previous.injections:freezeInjections(injectionsRef.current[session.id] || []);
    const frozenAnswers=previous?previous.assistantReferences:freezeAssistantReferences(assistantReferencesRef.current[session.id] || []);
    const frozenExercises=previous?previous.exerciseReferences:freezeExerciseReferences(exerciseReferencesRef.current[session.id] || []);
    if (previous) { const old = referenceConsumption.current.get(previous.operationId); if (old) { referenceConsumption.current.set(id, old); if (id !== previous.operationId) referenceConsumption.current.delete(previous.operationId); } } else rememberReferenceConsumption(id, session.id, activeId, selected);
    busyRef.current = id; setBusyId(id); setPartial(null); setAiError('');
    const sourceId = activeId; let succeeded = false;
    try { await prepareSource(sourceId); const result = await call<LearningSession>('exercise_generate', { sectionId: sourceId, sessionId: session.id, selection: selected, prompt, injections: frozen, assistantReferences: frozenAnswers, exerciseReferences: frozenExercises, userEntryId: previous?.userEntryId, operationId: id, resume: previous?.unknown === true }); updateTargetSession(result); setPracticeFailure(null); succeeded = true; }
    catch (error) { const details = error instanceof StationError ? error.details as { sendStage?: string; recovery?: string } | undefined : undefined; const recovery = details?.recovery === 'same-operation'; const unknown = recovery || details?.sendStage === 'unknown'; setPracticeFailure({ operationId: id, sourceId, selection: selected, prompt, injections: frozen, assistantReferences: frozenAnswers, exerciseReferences: frozenExercises, userEntryId: confirmedPracticeUsers.current.get(id) || previous?.userEntryId, unknown, recovery }); if (busyRef.current === id) setAiError(modelFailureMessage(error, 'generation')); }
    finally { const tree = await refreshTree(); await refreshAfterGeneration(session, false); if (busyRef.current === id) { busyRef.current = ''; setBusyId(''); } if (succeeded && activeIdRef.current === sourceId) { const child = tree?.nodes.filter(node => node.parentId === sourceId && node.kind === 'exercise').at(-1); if (child) await openSection(child.id); } }
  }
  async function submitAnswer(exercise: Exercise, answer: string) {
    const result = await call<LearningSession>('exercise_answer', { sectionId: activeId, sessionId: session.id, exerciseId: exercise.id, entryId: exercise.entryId, answer });
    updateTargetSession(result); return result;
  }
  async function saveReason(exercise: Exercise, reason: string) {
    const result = await call<LearningSession>('mistake_save', { sectionId: activeId, sessionId: session.id, exerciseId: exercise.id, question: exercise.prompt, reason });
    updateTargetSession(result); notify('错因已保存，下次可按情境找回。');
  }
  async function openLegacyHistory() { await readHistory('legacy_history_open'); }
  async function openSourceDocument(id:string){try{await call<Section>('section_open',{id});await openSection(id);}catch(error){notify(`来源暂不可用（${id}）：${errorText(error)}。原卡保留，未跳转到其他材料。`);}}
  async function openReviewSource(card: ReviewCard) {
    if (card.sourceDocumentId || card.sectionId) { await openSourceDocument(card.sourceDocumentId || card.sectionId); return; }
    if (!card.sessionId) { notify('这条历史记录缺少可打开的会话编号。'); return; }
    await openHistorySession(card.sessionId);
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
  async function refreshTree() {
    const sequence = ++treeSequence.current;
    try { const tree = await call<LibraryTreeSnapshot>('library_tree'); if (sequence === treeSequence.current) { nodesRef.current = tree.nodes; setBoot(old => ({ ...old, nodes: tree.nodes, treeWarnings: tree.warnings })); } return tree; }
    catch (error) { notify(`学习目录暂未刷新，已有内容仍保留：${errorText(error)}`); return undefined; }
  }
  function enterExplorations() {
    const previous = explorationStart.current;
    if (previous) {
      explorationBackups.current.add(previous);
      void previous.flush().then(() => explorationBackups.current.delete(previous)).catch(error => notify(`原探索草稿仍保留，关闭时将再次备份：${errorText(error)}`));
    }
    const id = makeId(); explorationEntryRef.current = id; setExplorationEntryId(id);
    if (isExplorationDocument(nodesRef.current, activeIdRef.current, explorationFolderId.current)) {
      activeIdRef.current = ''; setActiveId(''); setQuestion(''); setSelection('');
    }
    switchView('explorations');
  }
  async function startExploration(question: string, requestId: string, entryId: string) {
    const intent = navigationIntent.current;
    const stillAtEntry = () => intent === navigationIntent.current && viewRef.current === 'explorations' && explorationEntryRef.current === entryId;
    // Draft saving may finish after the user has already left this input.
    if (!stillAtEntry()) return { created: false, complete: false };
    if (busyRef.current || modelChangingRef.current) throw new Error('请先停止当前生成，再开始探索。');
    const result = await call<ExplorationNote>('exploration_note_create', { question, requestId });
    explorationFolderId.current = result.folderId;
    setSessions(old => ({ ...old, [result.document.id]: result.session }));
    await refreshTree();
    if (!stillAtEntry()) return { created: true, complete: false };
    const opened = openSection(result.document.id);
    const openedIntent = navigationIntent.current;
    await opened;
    if (openedIntent !== navigationIntent.current || activeIdRef.current !== result.document.id) return { created: true, complete: false };
    setRightbar(true);
    const existing=outgoingRef.current.find(item=>item.sendId===requestId);
    if(existing?.stage==='done')return {created:true,complete:true};
    const operationId=existing?retryOperation(existing,makeId()):`${requestId}-generation`;
    const item:OutgoingMessage={sendId:requestId,operationId,sessionId:result.session.id,sectionId:result.session.sectionId,documentId:result.document.id,detached:false,mode:'explore',content:question,selection:'',createdAt:new Date().toISOString(),userEntryId:result.userEntryId,stage:modelReady?'preparing':'failed',failureStage:existing?.failureStage||'unknown',error:modelReady?undefined:'问题已保存为本机笔记。配置模型后可重试生成，原问题不会再次保存。'};
    updateOutgoing(items=>[...items.filter(value=>value.sendId!==requestId),item]);
    if(!modelReady){notify('探索问题已保存到“探索”文件夹；连接获准使用的模型后继续。');return {created:true,complete:false};}
    busyRef.current=operationId;setBusyId(operationId);setPartial(null);setActiveRunId('');setAiError('');await executeSend(item,operationId);
    return {created:true,complete:outgoingRef.current.some(value=>value.sendId===requestId&&value.stage==='done')};
  }
  function subtreeIds(id:string) { const ids=new Set([id]);let changed=true;while(changed){changed=false;for(const node of nodesRef.current)if(node.parentId&&ids.has(node.parentId)&&!ids.has(node.id)){ids.add(node.id);changed=true;}}return ids; }
  async function prepareTrash(id:string){
    const ids=subtreeIds(id);
    for(const target of ids){
      const doc=docsRef.current[target];if(!doc)continue;
      await prepareSource(target);
      if(nativeAvailable){
        const latest=docsRef.current[target];const question=questionDrafts.current[target]||'';
        if(unbackedSectionHasInput(latest.draftLoaded,latest.body,latest.savedBody,question))throw new Error('该文件草稿尚未读取成功，请先处理草稿提示，当前输入仍保留。');
        if(latest.draftLoaded!==false)await call('draft_save',{key:`section:${target}`,data:{body:latest.body,question}});
      }
    }
    await flushExerciseDrafts();
  }
  async function afterTrash(receipt:TrashReceipt){setTrashNode(null);if(receipt.status!=='trashed'){notify('此操作记录已恢复，内容未再次收纳。');return;}const ids=subtreeIds(receipt.id);setDocs(old=>Object.fromEntries(Object.entries(old).filter(([id])=>!ids.has(id))));setSessions(old=>Object.fromEntries(Object.entries(old).filter(([id])=>!ids.has(id))));const tree=await refreshTree();if(ids.has(activeIdRef.current)){const next=tree?.nodes[0]?.id;if(next)await openSection(next);else{setActiveId('');setQuestion('');setSelection('');switchView('lesson');}}notify('已移至回收区，可按原编号恢复。');}
  async function readHistory(command: 'history_open' | 'legacy_history_open', args?: { sessionId: string }) {
    switchView('history'); setHistorySession(null); setHistoryError('');
    const intent = navigationIntent.current;
    try { const value = await call<LearningSession>(command, args); if (intent === navigationIntent.current) setHistorySession(value); }
    catch (error) { if (intent === navigationIntent.current) setHistoryError(`学习历史暂未打开：${errorText(error)}`); }
  }
  async function openHistorySession(id:string){await readHistory('history_open',{sessionId:id});}
  async function saveLinks(id: string, links: string[]) {
    if (busyRef.current) throw new Error('请先停止生成，再更改关联资料。');
    await prepareSource(id);
    linkWrites.current.add(id);
    try {
      const latest = docsRef.current[id];
      if (!latest || latest.saving) throw new Error('正文正在保存，请稍候再保存关联。');
      const section = await call<Section>('document_links', { id, links, expectedVersion: latest.section.version });
      setDocs(old => ({ ...old, [id]: { ...old[id], section } }));
      setLinkingMaterials(false); setContextRetry(old => old + 1);
    } finally { linkWrites.current.delete(id); }
  }
  async function acceptCreated(section: Section) { setCreatingNode(null); await refreshTree(); await openSection(section.id); }
  async function openExerciseEntry(entryId: string) {
    try { const document = await call<Section>('legacy_exercise_open', { sessionId: session.id, exerciseEntryId: entryId }); await refreshTree(); await openSection(document.id); }
    catch (error) { notify(`练习文档尚未打开：${errorText(error)}`); }
  }
  async function openAsset(id: string, assetId: string) {
    const token = ++assetSequence.current; setAssetLoading(true); setAssetPreview(null);
    try { const value = await call<CourseAssetPreview>('asset_read', { courseId: id, assetId }); if (token === assetSequence.current) setAssetPreview(value); }
    catch (error) { if (token === assetSequence.current) notify(`资料尚未打开：${errorText(error)}`); }
    finally { if (token === assetSequence.current) setAssetLoading(false); }
  }
  function insertAsset(citation: string) {
    const doc = docsRef.current[activeId]; if (!doc) return;
    const editor = editorRefs.current.get(activeId);
    if (editing && editor) editor.insertText(citation);
    else { changeBody(activeId, `${doc.body.trimEnd()}\n\n${citation}\n`); setEditedIds(old => new Set([...old, activeId])); setEditing(true); requestAnimationFrame(() => editorRefs.current.get(activeId)?.focus()); }
    notify('引用已加入正文草稿，将按当前保存状态写入本机。');
  }
  function appendExploration(text: string) { const doc = docsRef.current[activeId]; if (!doc || isContainerKind(doc.section.kind) || doc.section.kind === 'exercise') return; changeBody(activeId, `${doc.body.trimEnd()}\n\n${text}\n`); notify('已加入笔记正文草稿；助教原回复仍保留。'); }
  function openPanel(mode: Panel, preset?: string) { if (mode === 'practice') { void generateExercise(); return; } setRightbar(true); if (preset) setQuestion(preset); setTimeout(() => questionRef.current?.focus(), 0); }

  return <div style={{ '--content-font-size': `${font.fontSize}px`, '--assistant-width': `${assistantWidth.width}px`, '--sidebar-width': `${sidebarWidth.width}px`, '--ui-font-scale': font.fontSize / 16 } as React.CSSProperties} className={`app-shell ${navigation.collapsed ? 'sidebar-collapsed' : ''} ${!rightbar || view !== 'lesson' || isContainer ? 'rightbar-collapsed' : ''}`}>
    {isDemo && <div className="demo-banner">开发预览 · 示例材料，数据仅保留在内存 · 未连接本机服务或 AI</div>}
      <header className="topbar workspace-topbar">
        <div className="topbar-navigation"><button className="icon-button navigation-toggle" aria-label={navigation.collapsed ? '展开侧边栏' : '折叠为图标栏'} title={navigation.collapsed ? '展开左侧目录' : '收起左侧目录'} aria-expanded={!navigation.collapsed} onClick={()=>navigation.setCollapsed(!navigation.collapsed)}>{navigation.collapsed ? <PanelLeftOpen size={19}/> : <PanelLeftClose size={19}/>}</button><div className="breadcrumbs"><span>{view === 'lesson' ? (activeCourse?.title || '我的学习空间') : view === 'review' ? '复习与历史' : view === 'settings' ? '设置' : view === 'explorations' ? '探索' : view === 'history-index' ? '学习历史' : view === 'trash' ? '回收区' : view === 'history' ? '历史会话' : '学习待办'}</span><ChevronRight size={12} /><span className="breadcrumb-current">{view === 'lesson' ? '阅读与理解' : view === 'review' ? '把学过的串起来' : view === 'settings' ? '本机偏好' : view === 'explorations' ? '从问题开始' : view === 'history-index' ? '真实学习记录' : view === 'trash' ? '可恢复的学习内容' : view === 'history' ? '只读记录' : '接着完成下一步'}</span></div></div>
        <div className="topbar-actions"><button className={`icon-button navigation-toggle notice-button ${boot.notices.some(notice => !notice.read) ? 'has-notices' : ''}`} aria-label="通知" title="通知" onClick={() => setNotifications(!notifications)}><Clock3 size={19} /></button>{view === 'lesson' && <button className="icon-button navigation-toggle" aria-label={rightbar ? '收起学习助手' : '打开学习助手'} title={rightbar ? '收起右侧学习助手' : '打开右侧学习助手'} aria-expanded={rightbar} onClick={() => setRightbar(!rightbar)}>{rightbar ? <PanelRightClose size={19} /> : <PanelRightOpen size={19} />}</button>}</div>
      </header>
    <WorkspaceSidebar sidebarWidth={sidebarWidth.width} onSidebarWidth={sidebarWidth.setWidth} collapsed={navigation.collapsed} nodes={nodes} activeId={activeId} expandedIds={navigation.expandedIds} onExpanded={navigation.setExpandedIds} onOpen={id => { void openSection(id); }} onNew={setCreatingNode} onImport={() => setImporting(true)} onLearning={()=>switchView('lesson')} onExplorations={enterExplorations} onHistory={()=>switchView('history-index')} onTrash={()=>switchView('trash')} onTrashNode={node=>{if(busyRef.current){notify('请先停止生成，再将内容移至回收区。');return;}setTrashNode(node);}} blocked={!!busyId} onInject={id=>{void injectDocument(id);}} injectedIds={injections.map(item=>item.documentId)} onRemoveInjection={removeInjection} canInject={!!session.id} onReview={() => switchView('review')} onTasks={() => switchView('tasks')} onSettings={openSettings} view={view} loading={loading} dirtyIds={Object.entries(docs).filter(([,doc])=>doc.body!==doc.savedBody).map(([id])=>id)} taskCount={boot.tasks.filter(task=>!task.completed).length} modelReady={modelReady}/>

    <main className="main-area">

      {Object.entries(draftErrors).map(([key, message]) => <div className="connection-banner" role="status" key={key}><CircleHelp size={16} /><span>{key === 'workspace' ? '导入草稿' : '小节草稿'}备份尚未完成：{message} 当前输入仍保留在窗口中。</span><button onClick={() => { void retryBackup(key); }}>重试读取原草稿</button></div>)}
      {sidebarWidth.error && <div className="connection-banner" role="status">目录宽度尚未保存：{sidebarWidth.error}<button onClick={()=>{void sidebarWidth.retry().catch(()=>undefined);}}>重试读取并保存宽度</button></div>}
      {navigation.error && <div className="connection-banner" role="status">导航偏好尚未保存：{navigation.error}<button onClick={() => { void navigation.retry(); }}>重试读取偏好</button></div>}
      {boot.treeWarnings?.length ? <details className="tree-warnings"><summary>学习目录有 {boot.treeWarnings.length} 项提示</summary>{boot.treeWarnings.map((warning,index)=><p key={index}>{warning}</p>)}</details> : null}
      {bootError && <div className="connection-banner" role="status"><CircleHelp size={16} /><span>{bootError}</span><button onClick={() => { void load(); }}>重新连接</button></div>}
      {notifications && <div className="notice-popover"><div className="popover-heading">学习通知<button className="icon-button" aria-label="关闭通知" onClick={() => setNotifications(false)}><X size={15} /></button></div>{boot.notices.length ? boot.notices.map(notice => <article key={notice.id}><strong>{notice.title}</strong><p>{notice.body || '学习待办已到期。'}</p>{!notice.read && <button className="quiet-button" onClick={async () => { try { await call('notice_read', { id: notice.id }); setBoot(old => ({ ...old, notices: old.notices.map(item => item.id === notice.id ? { ...item, read: true } : item) })); } catch (error) { notify(errorText(error)); } }}>标为已读</button>}</article>) : <div className="subtle-empty">还没有通知。到期的学习待办会出现在这里。</div>}</div>}

      <div className="lesson-view" hidden={view !== 'lesson'}>
        {opening && !current ? <div className="center-state"><LoaderCircle className="spin" size={25} /><p>正在打开小节…</p></div> : current ? <>
          <div className="document-top"><div className="document-kicker"><span className="eyebrow">READ · THINK · CONNECT</span><div className={`save-status ${current.error ? 'status-error' : ''}`} aria-live="polite">{current.saving ? <><LoaderCircle size={12} className="spin" /> 保存中</> : current.error ? <><CircleHelp size={12} /> {current.conflict ? '文件有新版本' : '尚未保存'}</> : current.body !== current.savedBody ? <><span className="draft-dot" /> 本机草稿 · 等待保存</> : <><CheckCheck size={13} /> {isDemo ? '预览内容 · 仅在内存' : '已保存到本机'}</>}</div></div>
            <div className="document-title-row"><h1>{current.section.title.replace(/^\d+\s*[·.、-]\s*/, '')}</h1><button className="icon-button" title="查看来源" aria-label="查看来源" onClick={() => { const details = document.querySelector<HTMLDetailsElement>('.document-meta'); if (details) details.open = !details.open; }}><MoreHorizontal size={20} /></button></div>
            <div className="document-toolbar"><div className="segmented"><button className={!editing ? 'selected' : ''} onClick={() => setEditing(false)}><BookOpen size={14} />{current.section.kind === 'exercise' ? '做题' : '阅读'}</button><button className={editing ? 'selected' : ''} onClick={() => { if (!editedIds.has(activeId)) editorStartedAt.current[activeId] = performance.now(); setEditedIds(old => new Set([...old, activeId])); setEditing(true); }}><PencilLine size={14} />{current.section.kind === 'exercise' ? '编辑 Markdown' : '编辑'}</button></div>{current.section.kind !== 'exercise' && <button className="quiet-button" onClick={()=>setInsertingContent(true)}>插入内容</button>}<span className="toolbar-hint">{editing ? '直接编辑正文 · ⌘S 保存' : '选中一句话，继续追问'}</span><span className="word-count">{current.body.split(/\s+/).filter(Boolean).length} 词</span></div>
            <div className="document-actions">{isContainer ? <><button className="quiet-button" onClick={()=>setCreatingNode('section')}><Plus size={13}/>添加材料</button><button className="quiet-button" onClick={()=>setCreatingNode('folder')}><FolderOpen size={13}/>新建文件夹</button><button className="quiet-button" onClick={()=>setCreatingNode('section')}><Compass size={13}/>新建笔记</button></> : current.section.kind !== 'exercise' ? <><button className="quiet-button" disabled={!!busyId || !session.id} title="按右侧输入中的练习要求生成；未填写时使用当前材料和学习疑点" onClick={()=>{void generateExercise();}}><Sparkles size={13}/>生成练习子文档</button><button className="quiet-button" disabled={!!busyId || !session.id} onClick={()=>setManualExercise(true)}><PencilLine size={13}/>手动添加练习</button></> : current.section.parentId && <button className="quiet-button" onClick={()=>{void openSection(current.section.parentId!);}}><ArrowLeft size={13}/>回到来源</button>}{current.section.kind !== 'exercise' && !isContainer && <button className="quiet-button" disabled={!!busyId} title={busyId ? '请先停止生成，再更改关联材料' : '明确选择带入此探索的材料'} onClick={()=>setLinkingMaterials(true)}>关联材料 · {current.section.links?.length || 0}</button>}<button className={`quiet-button ${assetsOpen?'selected':''}`} aria-expanded={assetsOpen} onClick={()=>setAssetsOpen(!assetsOpen)}><Paperclip size={13}/>课程资料</button><button className="quiet-button" onClick={()=>setMovingNode(true)}>移动到…</button></div>
            <details className="document-meta"><summary><FileText size={12} />来源与文件信息 <ChevronDown size={12} /></summary><p>{current.section.sourcePath}</p><p>版本 {current.section.version} · 顶部元数据由系统维护，正文可自由编辑。</p><p>创建：{current.section.createdAt ? new Date(current.section.createdAt).toLocaleString('zh-CN') : '旧材料创建时间未知'} · 更新：{current.section.updatedAt ? new Date(current.section.updatedAt).toLocaleString('zh-CN') : '未知'}</p></details>
            {current.recovery && <div className="recovered-draft"><strong>已找回上次未写入文件的草稿</strong><p>当前显示你的草稿，原文件保持原样。确认后保存，或展开来源检查外部文件。</p><button className="quiet-button" onClick={() => { void save(activeId); }}>保存这份草稿</button><button className="quiet-button" onClick={async () => { try { setComparison(await call<Section>('section_open', { id: activeId })); } catch (error) { notify(errorText(error)); } }}>对照原文件</button></div>}
            {current.error && <div className="inline-error"><strong>{current.conflict ? '外部文件已更新，你的编辑仍在这里。' : '这次编辑还没有写入文件。'}</strong><p>{current.error}</p><div className="button-row">{current.conflict && <button onClick={async () => { try { setComparison(await call<Section>('section_open', { id: activeId })); } catch (error) { notify(errorText(error)); } }}>查看外部新版本</button>}{!current.conflict && <button onClick={() => { void save(activeId); }}>重新保存</button>}<button onClick={async () => { try { const result = await call<Section>('section_import', { title: `${current.section.title}（我的草稿）`, body: current.body }); notify('已另存一份草稿，原文件保留。'); setBoot(old => ({ ...old, courses: old.courses.map(course => course.id === result.courseId ? { ...course, sections: [...course.sections, { id: result.id, title: result.title }] } : course) })); setDocs(old => ({ ...old, [result.id]: { section: result, body: result.body, savedBody: result.body, saving: false, draftLoaded: result.draftReady !== false } })); setActiveId(result.id); } catch (error) { notify(errorText(error)); } }}>另存为新小节</button></div></div>}
          </div>
          <div className="document-scroll" onMouseUp={selectText} onKeyUp={selectText} onContextMenu={event=>{if(!current||isContainer||current.section.kind==='exercise')return;event.preventDefault();const chosen=window.getSelection()?.toString()||selection;setInjectionMenu({id:activeId,selection:chosen||undefined,x:Math.min(event.clientX,window.innerWidth-230),y:Math.min(event.clientY,window.innerHeight-130)});}}>
            {assetsOpen && courseId && <CourseAssets key={courseId} courseId={courseId} onInsert={insertAsset} onPreview={setAssetPreview}/>}
            {isContainer && <div className="folder-contents"><h2>{current.section.kind === 'course' ? '课程内容' : '文件夹内容'}</h2>{nodes.filter(node=>node.parentId===activeId).length ? nodes.filter(node=>node.parentId===activeId).map(node=><button key={node.id} className="folder-child" onClick={()=>{void openSection(node.id);}}><FileText size={15}/><span>{node.title}</span><ChevronRight size={14}/></button>) : <p className="muted">这里还没有内容。上方可以添加材料、文件夹或探索。</p>}</div>}
            {current.section.kind === 'exercise' && !editing && <section className="exercise-workspace" aria-label="练习作答"><p className="muted small">题目与评分读取原始学习记录，编辑下方 Markdown 不会更改评分依据。</p>{session.exercises.filter(exercise=>!current.section.exerciseEntryId || exercise.entryId===current.section.exerciseEntryId).map(exercise=><ExerciseCard key={exercise.id} exercise={exercise} sessionId={session.id} canGrade={modelReady} onRefresh={async()=>{updateTargetSession(await call<LearningSession>('session_refresh',{sectionId:session.sectionId,sessionId:session.id}));}} onReferenceSelection={(part,entryId,selected,position)=>{void quoteExercise(activeId,session.id,exercise.id,part,entryId,selected,position);}} onAnswer={answer=>submitAnswer(exercise,answer)} onReason={reason=>saveReason(exercise,reason)}/>)}<details className="exercise-markdown"><summary>明确展开参考与完整 Markdown</summary><Markdown text={current.body} courseId={courseId} onAsset={(id,assetId)=>{void openAsset(id,assetId);}}/></details></section>}
            {current.section.kind !== 'exercise' && !isContainer && session.exercises.length > 0 && <details className="legacy-exercises"><summary>本页练习记录 · {session.exercises.length} 题</summary>{[...new Map(session.exercises.filter(exercise=>exercise.entryId).map(exercise=>[exercise.entryId!,exercise])).values()].map(exercise=><button key={exercise.entryId} className="quiet-button" onClick={()=>{void openExerciseEntry(exercise.entryId!);}}>打开练习文档 · {exercise.prompt.slice(0,36)}</button>)}</details>}
            {!editing && current.section.kind !== 'exercise' && <div className="reading-content" tabIndex={0}><Markdown text={current.body} courseId={courseId} onAsset={(id,assetId)=>{void openAsset(id,assetId);}} className={current.body.split('\n')[0].replace(/^#\s+/, '').trim().toLowerCase() === current.section.title.replace(/^\d+\s*[·.、-]\s*/, '').trim().toLowerCase() ? 'hide-redundant-title' : ''} /><div className="reading-end"><span /><BookOpen size={15} /><span /></div></div>}
            {Object.entries(docs).filter(([id]) => editedIds.has(id)).map(([id, doc]) => <div key={id} className="document-editor" hidden={!editing || activeId !== id}><Suspense fallback={<div className="muted small">正在打开编辑器…</div>}><Editor ref={handle=>{if(handle)editorRefs.current.set(id,handle);else editorRefs.current.delete(id);}} initial={doc.body} onReady={() => { const started = editorStartedAt.current[id]; if (started !== undefined) { recordMetric('editor-ready', performance.now() - started); delete editorStartedAt.current[id]; } }} onChange={body => changeBody(id, body)} onSelection={value=>{if(activeIdRef.current===id && doc.section.kind !== 'exercise')setSelection(value);}} onSave={() => { void save(id); }} /></Suspense></div>)}
          </div>

        </> : <div className="welcome"><div className="welcome-symbol"><BookOpen size={32} strokeWidth={1.4} /></div><span className="eyebrow">YOUR OWN LEARNING SPACE</span><h1>从一小节开始，<br />把一个问题读明白。</h1><p>打开你的学习材料，边读边问。<br />小节、讲解和学过的错因，会自然留在一起。</p><button className="primary-button" onClick={() => setImporting(true)}><Plus size={16} />打开或粘贴我的材料</button><span className="welcome-support">支持 Markdown 与纯文本 · 原文保存在本机</span><button className="template-button" onClick={() => { if (!materialDraft.current.body) materialDraft.current = { title: "英语学习 · 每天的一小步（原创示例）", body: sampleBody }; else notify("已保留你未完成的导入草稿，请先保存它。"); setImporting(true); }}><BookOpen size={14} />使用英语学习模板<span>原创示例 · 可编辑 · 非 AI 生成</span></button>{openError && <div className="inline-error">{openError}<button onClick={() => { void openSection(activeId); }}>重试打开</button></div>}<div className="welcome-steps"><span><b>01</b> 读自己的材料</span><span><b>02</b> 随时问与练</span><span><b>03</b> 留下值得复习的</span></div></div>}
      </div>

      {view === 'review' && <ReviewPage nodes={nodes} onBack={()=>switchView('lesson')} onOpenSource={openReviewSource} onOpenDocument={openSourceDocument} notify={notify}/>}
      {view === 'tasks' && <div className="utility-page"><button className="text-back" onClick={() => switchView('lesson')}><ArrowLeft size={14} />回到小节</button><span className="eyebrow">ONE SMALL NEXT STEP</span><h1>给下次学习，留一步。</h1><p className="page-description">记下一个具体动作，让开始变得容易。到期状态由本机服务管理。</p><form className="task-form" onSubmit={event => { void createTask(event); }}><label>下一步做什么<input value={taskTitle} onChange={event => setTaskTitle(event.target.value)} placeholder="例如：再读一遍今天的转折句" required maxLength={200} /></label><div className="task-form-bottom"><div className="task-reminder-fields"><label className="checkbox-label"><input type="checkbox" checked={taskReminder} onChange={event => setTaskReminder(event.target.checked)} />到时间提醒我</label>{taskReminder && <label>提醒时间<input type="datetime-local" value={taskDue} onChange={event => setTaskDue(event.target.value)} required /></label>}</div><button className="primary-button" disabled={taskBusy || !taskTitle.trim()}>{taskBusy ? <LoaderCircle className="spin" size={15} /> : <Plus size={15} />}添加待办</button></div>{taskReminder && <p className="muted small">应用退出或电脑休眠时无法准点提醒；重开后会补入通知。飞书提醒未启用。</p>}</form>{taskError && <div className="inline-error" role="alert">{taskError}</div>}<div className="section-heading"><h2>学习待办 <span>{boot.tasks.filter(task => !task.completed).length}</span></h2></div>{!boot.tasks.length && <div className="empty-state"><ListTodo size={30} strokeWidth={1.2} /><h3>现在没有待办</h3><p>今天不必安排很多。一个能完成的小动作就够了。</p></div>}<div className="task-list">{boot.tasks.map(task => <div className={`task-item ${task.completed ? 'completed' : ''}`} key={task.id}><button className="task-check" aria-label={task.completed ? '标为未完成' : '完成待办'} onClick={() => { void toggleTask(task); }}>{task.completed && <Check size={14} />}</button><div><span>{task.title}</span>{task.dueAt && <small><Clock3 size={11} />{new Date(task.dueAt).toLocaleString('zh-CN', { month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit' })} · {task.reminderEnabled ? '提醒已开启' : '提醒未开启'}</small>}</div>{task.dueAt && !task.completed && <button className="task-reminder-toggle" onClick={() => { void toggleReminder(task); }}>{task.reminderEnabled ? '关闭提醒' : '开启提醒'}</button>}</div>)}</div></div>}
      <section className="utility-page exploration-records" hidden={view!=='explorations'}><button className="text-back" onClick={()=>switchView('lesson')}><ArrowLeft size={14}/>返回学习</button><ExplorationStart key={explorationEntryId} entryId={explorationEntryId} ref={explorationStart} onStart={startExploration}/></section>
      {view==='history-index'&&<SessionHistory onOpen={id=>{void openHistorySession(id);}} onLegacy={()=>{void openLegacyHistory();}}/>}
      {view==='trash'&&<TrashPanel onRestored={()=>{void refreshTree();notify('已恢复原文件、目录关联和学习记录。');}}/>}
      {view === 'history' && <section className="utility-page history-page"><button className="text-back" onClick={()=>switchView('history-index')}><ArrowLeft size={14}/>回到历史</button>{!historySession ? <p className={historyError ? 'inline-error' : 'context-loading'}>{historyError || '正在读取学习历史…'}</p> : <><h1>{historySession.sectionId ? '学习会话记录' : '旧版探索会话'}</h1><p className="muted">这里只读展示该真实会话，不会带入当前阅读文档。</p>{historySession.sectionId&&<button className="quiet-button" onClick={()=>{void openSection(historySession.sectionId);}}>打开来源文档</button>}{!historySession.messages.length && <p className="muted">这里还没有旧版会话记录。</p>}{historySession.messages.map(message=><article className={`message ${message.role}`} key={message.id} tabIndex={-1} data-assistant-entry={message.role==='assistant'?message.id:undefined} data-assistant-session={historySession.id}><div className="message-label">{message.role==='user'?'我的问题':message.role==='assistant'?'学习助手':'学习记录'}</div>{message.role==='assistant'&&message.runId&&<ToolActivityCards items={(historySession.toolCalls || []).filter(item=>item.runId===message.runId)} onOpen={object=>{void openToolObject(object);}}/>}{message.role === 'user' && <MessageReferences value={message.requestedContext} onOpen={id => { void openSourceDocument(id); }} onOpenAssistant={item => { void openAssistantQuote(item); }}/>}<div className={message.role === 'assistant' ? 'assistant-answer-content' : undefined} tabIndex={message.role === 'assistant' ? 0 : undefined} onContextMenu={event=>{if(message.role==='assistant')assistantMenu(event,message,historySession.id);}} onKeyDown={event=>{if(message.role==='assistant')assistantMenu(event,message,historySession.id);}}><Markdown text={message.content}/></div>{message.role === 'assistant' && <button type="button" className="message-save message-reply" aria-label="回复这条回答" onClick={() => { void replyToAssistant(message, historySession.id); }}><CornerUpLeft size={14}/>回复</button>}</article>)}<ToolActivityCards items={(historySession.toolCalls || []).filter(item=>!historySession.messages.some(message=>message.role==='assistant'&&message.runId===item.runId))} onOpen={object=>{void openToolObject(object);}}/></>}</section>}
      {view === 'settings' && <SettingsPage font={font} model={boot.model} dataPath={boot.dataPath} blocked={!!busyId || modelChanging} sessionId={session.id} onClose={() => switchView(settingsReturnView.current)} onSaved={model => setBoot(old => ({ ...old, model }))} />}
      <footer className="statusbar"><span><span className={`tiny-dot ${nativeAvailable ? '' : 'offline'}`} />{isDemo ? '开发预览' : nativeAvailable ? '本机工作空间' : '等待本机连接'}</span><span>{dirtyCount ? `${dirtyCount} 份草稿待保存` : '每一点理解，都值得留下'}</span><span>⌘ S 保存</span></footer>
    </main>

    {view === 'lesson' && !isContainer && <aside className="assistant-panel"><AssistantResizeHandle width={assistantWidth.width} onChange={assistantWidth.setWidth} /><div className="assistant-heading"><span className="assistant-mark"><Sparkles size={17} /></span><div><h2>一起读明白</h2><p>顺着你的问题，往前一步</p></div></div>
      {assistantWidth.error && <div className="inline-error" role="status">面板宽度尚未保存：{assistantWidth.error}<button onClick={() => { void assistantWidth.flush().catch(() => undefined); }}>重试保存</button></div>}
      <SelectionContextTray references={composerRefs} busy={!!busyId} onRemove={removeComposerReference} onAsk={() => openPanel('explain', '请帮我理解这段内容。')} onPractice={() => openPanel('practice')} onReview={quote => setCardDraft({ front: quote, back: '', sessionId: session.id, sectionId: session.sectionId })}/>
      <div className="assistant-scroll">
        <RequestContextView value={contextView.sessionId && contextView.sessionId !== session.id ? { status: 'loading' } : contextView} independent={current?.section.kind === 'exploration'} onRetry={() => setContextRetry(old => old + 1)} />

        {boot.recovery && <details className="recovery-message"><summary>上次生成未完成 · 查看模型原返回（可能含参考）<ChevronDown size={12} /></summary><p>{boot.recovery.message || '应用关闭时生成尚未完成，已保存内容仍保留。'}</p>{boot.recovery.content ? <Markdown text={boot.recovery.content} /> : <p>尚未收到正文。可以重新发送问题。</p>}</details>}
        {!modelReady && <div className="model-prompt"><span className="model-prompt-icon"><Sparkles size={16} /></span><div><strong>{modelBlocked ? '当前模型用途受限' : '连接自己的模型，开始对话'}</strong><p>{modelBlocked ? '可在设置中查看限制或配置获准使用的自定义服务。' : '可手动填写服务地址、协议和模型名称。'}</p><button onClick={() => openSettings()}>设置模型连接 <ArrowRight size={13} /></button></div></div>}
        <>
          {!conversation.length && <div className="assistant-empty"><div className="little-line" /><h3>{current?.section.kind === 'exploration' ? '把好问题写成自己的理解。' : '哪里卡住了？'}</h3><p>{current?.section.kind === 'exploration' ? '左侧探索正文由你编辑；助教帮你比较、追问和澄清。' : '词义、句子关系，或一整段的意思，都可以慢慢问。'}</p><div className="starter-questions"><button disabled={!canCompose} onClick={() => { updateComposer('请带我梳理本页的核心问题，先从一个问题开始。'); questionRef.current?.focus(); }}>带我梳理这一页 <ArrowRight size={13} /></button><button disabled={!canCompose} onClick={() => { updateComposer('这份学习材料里，有哪些容易理解错的地方？'); questionRef.current?.focus(); }}>找找容易理解错的地方 <ArrowRight size={13} /></button></div></div>}
          <div className="messages">{displayedMessages.map(message => <article className={`message ${message.role}`} key={message.clientMessageId || message.id} tabIndex={-1} data-assistant-entry={message.role==='assistant'?message.id:undefined} data-assistant-session={session.id}><div className="message-label">{message.role === 'user' ? '我的问题' : message.role === 'note' ? <><FileText size={12} />学习记录 · AI整理{message.needsConfirmation && <span className="pending-label">待确认</span>}</> : <><Sparkles size={12} />学习助手</>}</div>{message.role==='assistant'&&message.runId&&<ToolActivityCards items={toolCalls.filter(item=>item.runId===message.runId)} onOpen={object=>{void openToolObject(object);}}/>}{message.role === 'user' && <MessageReferences value={message.requestedContext} onOpen={id => { void openSourceDocument(id); }} onOpenAssistant={item => { void openAssistantQuote(item); }}/>}<div className={message.role === 'assistant' ? 'assistant-answer-content' : undefined} tabIndex={message.role === 'assistant' ? 0 : undefined} onContextMenu={event=>{if(message.role==='assistant')assistantMenu(event,message,session.id);}} onKeyDown={event=>{if(message.role==='assistant')assistantMenu(event,message,session.id);}}><Markdown text={message.content} courseId={courseId} onAsset={(id,assetId)=>{void openAsset(id,assetId);}} /></div>{message.role === 'assistant' && <button type="button" className="message-save message-reply" aria-label="回复这条回答" onClick={() => { void replyToAssistant(message, session.id); }}><CornerUpLeft size={14}/>回复</button>}{message.outgoing && message.outgoing.stage !== 'done' && <div className={`message-delivery ${message.outgoing.stage}`} role="status"><span>{sendStageLabel(message.outgoing)}</span>{message.outgoing.error && <p>{message.outgoing.error}</p>}{message.outgoing.stage === 'failed' && <button className="quiet-button" disabled={!!busyId || modelChanging} onClick={() => { void retrySend(message.outgoing!.sendId); }}>{message.outgoing.failureStage === 'unknown' ? '核对生成状态' : message.outgoing.userEntryId ? '重试生成' : '重试保存并生成'}</button>}</div>}{message.role === 'assistant' && (message.context ? <RequestContextView value={{status:'ready',actual:true,context:message.context}} independent={false} label="此条回复的实际来源" onRetry={()=>undefined}/> : <p className="muted small">此条历史回复未记录来源快照。</p>)}{message.sources?.length ? <SourceList sources={message.sources} /> : null}{message.role === 'assistant' && <button className="message-save" onClick={() => setCardDraft({ front: selection || '这一页中值得回忆的问题', back: message.content, sessionId: session.id, sectionId: session.sectionId })}><Plus size={12} />留作复习</button>}{message.role === 'assistant' && current && current.section.kind !== 'exercise' && <button className="message-save" onClick={()=>appendExploration(message.content)}><PencilLine size={12}/>加入笔记正文</button>}</article>)}</div>{conversation.length > 30 && <p className="muted small">显示最近 30 条。完整记录保存在学习文件中。</p>}
        </>
        <ToolActivityCards items={toolCalls.filter(item=>!repliedRuns.has(item.runId))} liveRunId={busyId ? activeRunId : undefined} onOpen={object => { void openToolObject(object); }} />
        {partial && partial.mode !== 'practice' && partial.sessionId === session.id && busyId && <article className="message assistant partial-message"><div className="message-label"><Sparkles size={12} />正在回复</div><Markdown text={partial.content} /></article>}{partial?.mode === 'practice' && partial.sessionId === session.id && !busyId && aiError && <details className="recovery-message"><summary>查看练习模型原返回（含参考）</summary><pre>{partial.content}</pre></details>}{busyId && <div className="generating" role="status"><LoaderCircle size={15} className="spin" /><div>{partial?.mode === 'practice' ? '正在接收、校验并保存练习…' : '正在等待模型回复…'}<small>来源以核对后的实际清单为准</small></div></div>}{aiError && <div className="inline-error" role="alert"><strong>这次没有得到回复</strong><p>{aiError}</p><span>输入仍保留，可以修改后重试。</span>{practiceFailure?.sourceId === activeId && <button className="quiet-button" disabled={!!busyId} onClick={()=>{void generateExercise(true);}}>{practiceFailure.unknown ? practiceFailure.recovery ? '继续完成保存' : '核对上次练习生成' : '重试生成练习'}</button>}</div>}<div ref={chatEnd} />
      </div>
      <div className="composer-area"><div className="composer"><ComposerReferenceChips references={composerRefs} busy={!!busyId} onRemove={removeComposerReference} onOpenAssistant={item=>{void openAssistantQuote(item);}}/><textarea ref={questionRef} aria-label="向学习助手提问" placeholder={current ? '写下问题，或生成练习的要求…' : '先打开一份材料或探索'} value={composerQuestion} disabled={!canCompose} rows={3} onChange={event => updateComposer(event.target.value)} onCompositionStart={() => { cancelAnimationFrame(composerCommitFrame.current); composerComposing.current = true; }} onCompositionEnd={() => { composerCommitFrame.current = requestAnimationFrame(() => { composerComposing.current = false; }); }} onBlur={() => { cancelAnimationFrame(composerCommitFrame.current); composerComposing.current = false; }} onKeyDown={event => { const action = composerKeyAction(event.nativeEvent, composerComposing.current); if (action === 'native') return; event.preventDefault(); if (action === 'send') void ask(); }} /><div className="composer-bottom"><button type="button" className="save-question" disabled={!composerQuestion.trim() || localQuestionSaving} onClick={() => { void saveLocalQuestion(); }}>{localQuestionSaving ? <LoaderCircle size={11} className="spin" /> : <Plus size={11} />}保存困惑</button><ChatModelPicker model={boot.model} busy={!!busyId || modelChanging} onApply={changeModel} onSettings={openSettings} /><button type="button" onClick={()=>{if(busyId)void cancel();else void ask();}} className="send-button" aria-label={busyId ? '停止生成' : modelReady ? '发送问题' : '配置模型后提问'} disabled={!busyId && (!composerQuestion.trim() || modelChanging)}>{busyId ? <Square size={15} /> : <ArrowUp size={17} />}</button></div></div></div>
    </aside>}

    {exerciseQuote && <Dialog title="引用练习分区" description="从当前题的规范原文选择连续内容，不切换编辑页面。" onClose={()=>{exerciseQuoteSequence.current++;setExerciseQuote(null);}}><ExerciseQuote source={exerciseQuote} onAdd={selected=>addExerciseQuote(exerciseQuote,selected)}/></Dialog>}

    {assistantReplyMenu && <AssistantReplyMenu value={assistantReplyMenu} onClose={closeAssistantMenu} onReply={()=>{const value=assistantReplyMenu;closeAssistantMenu(false);void replyToAssistant(value.message,value.sessionId,value.selection);}}/>}

    {insertingContent && current && <Dialog title="插入 HTML、网页或视频" description="原文与链接随笔记一起保存，阅读时可打开隔离预览。" onClose={()=>setInsertingContent(false)}><InsertContent onInsert={text=>{const editor=editing?editorRefs.current.get(activeId):undefined;if(editor)editor.insertText(text);else changeBody(activeId,current.body+text);setInsertingContent(false);}}/></Dialog>}
    {importing && <ImportDialog nodes={nodes} defaultParentId={isContainer ? activeId : current?.section.parentId || undefined} draft={materialDraft} onDraftLoaded={loaded => { workspaceDraftLoaded.current = loaded; if (loaded) clearDraftError('workspace'); }} onClose={() => setImporting(false)} onImported={async section => { if (workspaceDraftLoaded.current) { materialDraft.current = { title: '', body: '' }; if (nativeAvailable) void call('draft_save', { key: 'workspace', data: { importTitle: '', importBody: '' } }).catch(error => reportDraftError('workspace', error)); } setImporting(false); await acceptCreated(section); }} />}

    {trashNode && <Dialog title="移至可恢复回收区" description="先核对整棵目录和会话范围，再确认；不会永久删除文件。" onClose={()=>setTrashNode(null)}><TrashConfirmation node={trashNode} prepare={()=>prepareTrash(trashNode.id)} onDone={receipt=>{void afterTrash(receipt);}}/></Dialog>}
    {creatingNode && <Dialog title="新建学习内容" description="课程、目录、正文都保存在本机；之后可以整理到其他目录。" onClose={()=>setCreatingNode(null)}><CreateNodeForm nodes={nodes} defaultKind={creatingNode} parentId={isContainer ? activeId : current?.section.parentId || undefined} onCreated={section=>{void acceptCreated(section);}}/></Dialog>}
    {linkingMaterials && current && <Dialog title="关联本页的材料" description="保存成功后才会改变实际来源；提问、选区与学习记录仍属于当前笔记。" onClose={()=>setLinkingMaterials(false)}><LinkedMaterials section={current.section} nodes={nodes} onSave={links=>saveLinks(current.section.id,links)}/></Dialog>}
    {movingNode && current && <Dialog title="整理到其他目录" description="只改变持久目录归属，稳定编号与学习记录保持关联。" onClose={()=>setMovingNode(false)}><MoveNodeForm node={current.section} nodes={nodes} onMoved={section=>{setMovingNode(false);setDocs(old=>({...old,[section.id]:{...old[section.id],section,savedBody:section.body}}));void refreshTree();}}/></Dialog>}
    {(assetLoading || assetPreview) && <Dialog title={assetPreview?.asset.fileName || '正在读取本机资料'} description="课程资料只读预览，不执行代码、不载入远程内容。" onClose={()=>{assetSequence.current++;setAssetLoading(false);setAssetPreview(null);}}>{assetLoading ? <p className="context-loading"><LoaderCircle size={15} className="spin"/>正在读取本课程资料…</p> : assetPreview && <AssetPreviewContent preview={assetPreview}/>}</Dialog>}
    {objectPreview && <Dialog title={objectPreview.kind === 'todo' ? '已保存的待办' : objectPreview.title} description="从本机真实记录打开，未重新调用模型或工具。" onClose={() => setObjectPreview(null)}>{objectPreview.kind === 'todo' ? <><h3>{objectPreview.title}</h3><p>{objectPreview.done ? '已完成' : '待完成'} · {objectPreview.reminderEnabled ? '提醒已开启' : '提醒关闭'}</p>{objectPreview.dueAt && <p>{new Date(objectPreview.dueAt).toLocaleString('zh-CN')}</p>}<button className="quiet-button" onClick={() => { setObjectPreview(null); switchView('tasks'); }}>到学习待办</button></> : <><Markdown text={objectPreview.front || objectPreview.body || ''} />{objectPreview.back && <Markdown text={objectPreview.back} />}</>}</Dialog>}
    {backupConflict && <Dialog title="找回了之前的草稿" description="旧草稿还没有被覆盖。请对照正文与未发送的问题，再选择保留哪份输入。" onClose={() => setBackupConflict(null)}><div className="conflict-compare">{[{ label: '当前窗口输入', data: currentDraftData(backupConflict.key) }, { label: '找回的旧草稿', data: backupConflict.data }].map(item => <div key={item.label}><h3>{item.label}</h3>{backupConflict.key === 'workspace' ? <><h4>小节名称</h4><p>{item.data.importTitle || '（空）'}</p><h4>正文</h4><Markdown text={item.data.importBody || '（空）'} /></> : <><h4>正文</h4><Markdown text={item.data.body || '（空）'} /><h4>未发送的问题</h4><p className="draft-question">{item.data.question || '（空）'}</p></>}</div>)}</div><div className="dialog-footer"><button className="quiet-button" onClick={() => { void resolveBackup(true); }}>使用找回的草稿</button><button className="primary-button" onClick={() => { void resolveBackup(false); }}>保留当前输入并备份</button></div></Dialog>}
    {manualExercise && <ManualExerciseDialog sectionId={activeId} sessionId={session.id} onClose={() => setManualExercise(false)} onSaved={result => { updateTargetSession(result.session); setManualExercise(false); void acceptCreated(result.document); notify('练习子文档已保存，可以开始作答。'); }} />}
    {comparison && <Dialog title="对照外部的新版本" description="左侧是仍保留的本地编辑，右侧是当前文件。关闭对照后可以继续整理，或将草稿另存为新小节。" onClose={() => setComparison(null)}><div className="conflict-compare"><div><h3>我的编辑</h3><Markdown text={current?.body || ''} /></div><div><h3>外部文件</h3><Markdown text={comparison.body} /></div></div></Dialog>}
    {cardDraft && <CardDialog draft={cardDraft} sectionId={cardDraft.sectionId ?? session.sectionId} onClose={() => setCardDraft(null)} onSaved={card => { setSessions(old => ({ ...old, [activeId]: { ...session, cards: [...session.cards, card] } })); setCardDraft(null); notify('已留作复习，可从左下角“复习与历史”找回。'); }} />}
    {injectionMenu&&<div className="tree-context-menu" role="menu" aria-label="材料上下文操作" style={{left:Math.max(8,injectionMenu.x),top:Math.max(8,injectionMenu.y)}} onKeyDown={event=>{if(event.key==='Escape')setInjectionMenu(null);}}><button role="menuitem" disabled={!session.id} onClick={()=>{if(injectionMenu.exercise){const item=injectionMenu.exercise;void quoteExercise(injectionMenu.id,item.sessionId,item.questionId,item.part,item.entryId,injectionMenu.selection||'');}else void injectDocument(injectionMenu.id,injectionMenu.selection);setInjectionMenu(null);}}>{injectionMenu.selection?'将选段加入本次上下文':'将全文加入本次上下文'}</button>{!injectionMenu.exercise&&injections.some(item=>item.documentId===injectionMenu.id)&&<button role="menuitem" onClick={()=>{removeInjection(injectionMenu.id);setInjectionMenu(null);}}>从本次上下文移除</button>}<button role="menuitem" onClick={()=>setInjectionMenu(null)}>关闭</button></div>}
    {toast && <div className="toast" role="status"><Check size={16} /><span>{toast}</span><button className="icon-button" aria-label="关闭提示" onClick={() => setToast('')}><X size={14} /></button></div>}
  </div>;
}

function SourceList({ sources }: { sources: Source[] }) { return <details className="source-list"><summary><FileText size={12} />查看 {sources.length} 个上下文来源 <ChevronDown size={12} /></summary>{sources.map((source, index) => <div className="source-details" key={index}><strong>{source.title}</strong><p>{source.path}</p>{source.excerpt && <blockquote>{source.excerpt}</blockquote>}</div>)}</details>; }
function Dialog({ title, description, onClose, children }: { title: string; description: string; onClose: () => void; children: React.ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => { const previous = document.activeElement as HTMLElement; ref.current?.querySelector<HTMLElement>('input,textarea,button')?.focus(); return () => previous?.focus(); }, []);
  return <div className="dialog-backdrop" onMouseDown={event => { if (event.target === event.currentTarget) onClose(); }}><div className="dialog" role="dialog" aria-modal="true" aria-labelledby="dialog-title" ref={ref} onKeyDown={event => { if (event.key !== 'Tab') return; const elements = ref.current?.querySelectorAll<HTMLElement>('button:not(:disabled),input:not(:disabled),textarea:not(:disabled),select:not(:disabled),[tabindex="0"]'); if (!elements?.length) return; const first = elements[0]; const last = elements[elements.length - 1]; if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); } else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); } }}><button className="dialog-close icon-button" aria-label="关闭弹窗" onClick={onClose}><X size={19} /></button><span className="dialog-eyebrow">LEARNING STATION</span><h2 id="dialog-title">{title}</h2><p className="dialog-description">{description}</p>{children}</div></div>;
}
function ImportDialog({ onClose, onImported, draft, onDraftLoaded, nodes, defaultParentId }: { nodes: LibraryNode[]; defaultParentId?: string; onClose: () => void; onImported: (section: Section) => void; draft: React.RefObject<{ title: string; body: string }>; onDraftLoaded: (loaded: boolean) => void }) {
  const [parentId, setParentId] = useState(defaultParentId || ''); const importRequestId = useRef(crypto.randomUUID());
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
  async function submit(event: React.FormEvent) { event.preventDefault(); if (!body.trim() || busy) return; setBusy(true); setError(''); try { onImported(await call<Section>('section_import', { title: title.trim() || '未命名小节', body, parentId: parentId || undefined, requestId: importRequestId.current })); } catch (error) { setError(errorText(error)); } finally { setBusy(false); } }
  return <Dialog title="打开一份自己的材料" description="一篇短文、一页笔记，或今天要读的小节。系统会自动保存为课程材料。" onClose={onClose}><form onSubmit={event => { void submit(event); }}><input ref={file} type="file" accept=".md,.markdown,.txt,text/plain,text/markdown" hidden onChange={async event => { const selected = event.target.files?.[0]; if (!selected) return; edited.current = true; if (selected.size > 1_500_000) { setError('这份文件超过 1.5MB，请先按小节拆分。'); return; } try { setBody(await selected.text()); if (!title) setTitle(selected.name.replace(/\.(md|markdown|txt)$/i, '')); } catch { setError('无法读取这份文件，请确认文件可用后重试，也可以粘贴正文。'); } }} /><button type="button" className="file-picker" onClick={() => file.current?.click()}><FolderOpen size={20} /><span>选择 Markdown 或文本文件<small>也可以直接粘贴在下方</small></span><Plus size={17} /></button><label>放在<select aria-label="导入目标目录" value={parentId} onChange={event=>setParentId(event.target.value)} disabled={busy}><option value="">学习空间根目录</option>{nodes.filter(node=>node.kind!=='exercise').map(node=><option key={node.id} value={node.id}>{node.title}</option>)}</select></label><label>小节名称<input value={title} onChange={event => { edited.current = true; setTitle(event.target.value); }} placeholder="例如：阅读 · Daily habits" maxLength={200} /></label><label>小节正文<textarea className="import-body" value={body} onChange={event => { edited.current = true; setBody(event.target.value); }} placeholder="把自己的学习材料粘贴在这里…" required rows={8} /></label>{error && <div className="inline-error" role="alert">{error}{savedImport ? <><p>旧名称：{savedImport.data.importTitle || '（空）'}</p><details><summary>查看旧正文</summary><Markdown text={savedImport.data.importBody || '（空）'} /></details><div className="button-row"><button type="button" onClick={() => { setTitle(savedImport.data.importTitle || ''); setBody(savedImport.data.importBody || ''); loadedRef.current = true; setDraftLoaded(true); onDraftLoaded(true); setSavedImport(null); setError(''); }}>恢复旧导入草稿</button><button type="button" onClick={() => { loadedRef.current = true; setDraftLoaded(true); onDraftLoaded(true); setSavedImport(null); setError(''); }}>保留当前输入并备份</button></div></> : !draftLoaded && <button type="button" onClick={() => { void restoreDraft(); }}>重试读取导入草稿</button>}</div>}<div className="dialog-footer"><span>正文可随时编辑</span><button className="primary-button" disabled={busy || !body.trim()}>{busy ? <LoaderCircle className="spin" size={15} /> : <ArrowRight size={15} />}{busy ? '正在保存到本机…' : '保存并开始阅读'}</button></div></form></Dialog>;
}

function CardDialog({ draft, sectionId, onClose, onSaved }: { draft: CardDraft; sectionId: string; onClose: () => void; onSaved: (card: ReviewCard) => void }) {
  const [front, setFront] = useState(draft.front); const [back, setBack] = useState(draft.back); const [busy, setBusy] = useState(false); const [error, setError] = useState('');
  async function submit(event: React.FormEvent) { event.preventDefault(); setBusy(true); setError(''); try { onSaved(await call<ReviewCard>('card_create', { sectionId, sessionId: draft.sessionId, front, back })); } catch (error) { setError(errorText(error)); } finally { setBusy(false); } }
  return <Dialog title="留一个问题，给未来的自己" description="原文来源会自动关联。可以只保留选句，也可以写下自己的理解。" onClose={onClose}><form onSubmit={event => { void submit(event); }}><label>下次想回忆什么<textarea value={front} onChange={event => setFront(event.target.value)} rows={4} required /></label><label>我的理解或提醒（可选）<textarea value={back} onChange={event => setBack(event.target.value)} rows={4} placeholder="用自己的话，写下你希望记住的…" /></label>{error && <div className="inline-error" role="alert">{error}</div>}<div className="dialog-footer"><span>之后可导出 CSV</span><button className="primary-button" disabled={busy || !front.trim()}>{busy ? <LoaderCircle className="spin" size={14} /> : <Layers2 size={14} />}保存到复习</button></div></form></Dialog>;
}

function ManualExerciseDialog({ sectionId, sessionId, onClose, onSaved }: { sectionId: string; sessionId: string; onClose: () => void; onSaved: (result: { session: LearningSession; document: Section }) => void }) {
  const exerciseKey = useRef(crypto.randomUUID().replaceAll('-', ''));
  const [prompt, setPrompt] = useState(''); const [reference, setReference] = useState(''); const [exact, setExact] = useState(false); const [busy, setBusy] = useState(false); const [error, setError] = useState('');
  async function submit(event: React.FormEvent) { event.preventDefault(); setBusy(true); setError(''); try { onSaved(await call<{ session: LearningSession; document: Section }>('exercise_create_manual', { sectionId, sessionId, prompt, reference, exact, id: exerciseKey.current })); } catch (error) { setError(errorText(error)); } finally { setBusy(false); } }
  return <Dialog title="添加一道自己的练习" description="题目可以离线保存；主观题提交后需要 AI 批改，短答案可在本机核对。" onClose={onClose}><form onSubmit={event => { void submit(event); }}><label>题目<textarea aria-label="手动练习题目" rows={3} value={prompt} onChange={event => setPrompt(event.target.value)} required placeholder="例如：解释这一现象的原因，并给出依据…" /></label><label>参考答案或检查要点<textarea aria-label="练习参考答案" rows={3} value={reference} onChange={event => setReference(event.target.value)} required placeholder="写下对照答案，或你想检查的要点…" /></label><label className="checkbox-label"><input type="checkbox" checked={exact} onChange={event => setExact(event.target.checked)} />按唯一短答案比较（忽略大小写与多余空格）</label><p className="muted small">主观题须 AI 批改；未配置模型时保留已提交答案并提示待批改。题目与参考由你提供。</p>{error && <div className="inline-error" role="alert">{error}</div>}<div className="dialog-footer"><span>来源自动关联当前小节</span><button className="primary-button" disabled={busy || !prompt.trim() || !reference.trim()}>{busy ? <LoaderCircle className="spin" size={14} /> : <Plus size={14} />}保存题目</button></div></form></Dialog>;
}
