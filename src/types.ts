export type Source = { title: string; path?: string; excerpt?: string };
export type SectionSummary = { id: string; title: string };
export type Course = { id: string; title: string; sections: SectionSummary[] };
export type Section = { id: string; courseId: string; title: string; body: string; version: string; sourcePath: string; updatedAt?: string; createdAt?: string | null; timestampSource?: string; kind?: LibraryNodeKind; parentId?: string | null; originSessionId?: string; exerciseEntryId?: string; importWarning?: string; links?: string[]; draftReady?: boolean; draftError?: string };
export type Message = { id: string; clientMessageId?: string; runId?: string; role: 'user' | 'assistant' | 'note'; needsConfirmation?: boolean; content: string; createdAt?: string; sources?: Source[]; context?: RequestContext; requestedContext?: RequestedContext };
export type ToolActivity = { id: string; toolCallId: string; runId: string; sessionId: string; name: string; status: 'running' | 'succeeded' | 'failed'; title: string; summary: string; startedAt: string; updatedAt: string; persisted: boolean; object?: { kind: 'todo' | 'learning-entry' | 'review-card' | 'exercise'; id: string; sessionId: string; sectionId: string }; errorCode?: string };
export type Rubric = { criteria: { id: string; label: string; description: string; maxPoints: number }[] };
export type GradeResult = { attemptId: string; exerciseId: string; overallFeedback: string; score: null | { earned: number; max: number }; criteria: { criterionId: string; earned: number; feedback: string; evidence: string[] }[]; annotations: { quote: string; occurrence: number; issue: string; reason: string; suggestion: string; criterionId?: string; anchor: null | { start: number; end: number } }[]; missingPoints: { criterionId?: string; description: string; suggestion: string }[]; weakPoints: string[]; rubricVersion: string; answerVersion: string };
export type GradeRun = { id: string; sessionId: string; attemptId: string; status: 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled' | 'interrupted'; result: GradeResult | null; error: null | { code: string; message: string }; model: string; updatedAt: string };
export type GradingSettings = { model: string | null; effectiveModel: string; configured: boolean; usesCurrentProvider: true };
export type Exercise = { sessionId?: string; id: string; entryId?: string; prompt: string; answer?: string; feedback?: string; correct?: boolean; format?: 'exact' | 'self'; reference?: string; explanation?: string; attemptId?: string; feedbackEntryId?: string; rubric?: Rubric; aiGrade?: GradeResult; cause?: string; causeUpdatedAt?: string };
export type Mistake = { id: string; question: string; reason: string; sectionId: string; createdAt?: string };
export type ReviewSource = { reviewItemId: string; sessionId: string; sectionId: string | null; sectionTitle: string; courseId: string | null; courseTitle: string | null };
export type ReviewCard = { id: string; sessionId?: string; front: string; back: string; sectionId: string; source?: string; dueAt?: string; courseId?: string | null; courseTitle?: string | null; sectionTitle?: string; sourceDocumentId?: string | null; reviewState?: { status: 'new' | 'again' | 'understood'; reviewedAt: string | null } };
export type AnkiConnection = { connected: boolean; code: string; message: string; version?: number; decks: { id: number; name: string }[] };
export type AnkiCard = { cardId: number; deckName: string; question: string; reviewToken: string; nextReviews: string[]; source: ReviewSource | null };
export type AnkiOperation = { requestId: string; status: 'succeeded' | 'failed' | 'unknown'; cardId?: number; noteIds?: number[]; error?: { code: string; message: string } };
export type CitationLocator = { startChar: number; endChar: number };
export type AssistantReference = { sessionId: string; entryId: string; runId?: string; textHash: string; selection: string; locator?: CitationLocator };
export type ExerciseReferencePart = 'question' | 'attempt' | 'feedback' | 'reference';
export type ExerciseReference = { sessionId: string; documentId: string; exerciseEntryId: string; questionId: string; part: ExerciseReferencePart; entryId: string; textHash: string; selection: string; locator?: CitationLocator };
export type ExerciseReferenceSource = Omit<ExerciseReference, 'selection' | 'locator'> & { title: string; documentVersion: string; text: string; sourceId: string };
export type FocusInput = { documentId: string; expectedBody: string; expectedVersion?: string; view: 'read' | 'edit' | 'exercise' | 'asset'; ranges: { start: number; end: number }[]; questionId?: string; assetId?: string };
export type FocusSnapshot = { id: string; courseId: string; sessionId?: string; repositoryVersion?: 2; documentId: string; sourceDocumentId: string; path: string; type: 'markdown' | 'text' | 'exercise' | 'image' | 'pdf'; commitHash: string; materialTreeOid: string; excludedFiles: { assetId: string; fileName: string; reason: string }[]; documentVersion: string; view: FocusInput['view']; binary?: boolean; paragraphs: { start: number; end: number; text: string; startChar?: number; endChar?: number; totalChars?: number; totalBytes?: number }[]; exercise?: { questionId: string; number: number; answered: boolean; graded: boolean }; asset?: { id: string; mime: string; size: number } };
export type RequestedContext = { focus?: FocusSnapshot; documentId: string; selection: string; injections: ContextInjection[]; assistantReferences?: AssistantReference[]; exerciseReferences?: ExerciseReference[] };
export type ContextInjection = { documentId: string; expectedVersion: string; selection?: string; locator?: CitationLocator };
export type LearningSession = { title?: string; version?: string; courseId?: string; legacySessionIds?: string[]; exerciseSessionId?: string; id: string; sectionId: string; messages: Message[]; toolCalls?: ToolActivity[]; sources?: Source[]; contextAssistantReferenceManifest?: ContextManifestItem[]; contextExerciseReferenceManifest?: ContextManifestItem[]; exercises: Exercise[]; mistakes: Mistake[]; cards: ReviewCard[] };
export type Task = { id: string; title: string; dueAt?: string; completed: boolean; reminderEnabled?: boolean };
export type Notice = { id: string; title: string; body?: string; read: boolean };
export type UiPreferences = { fontSize: number; assistantWidth?: number; sidebarWidth?: number; sidebarCollapsed?: boolean; expandedNodeIds?: string[]; toolAuthorizationTier?: string };
export type ContextManifestItem = { id: string; path: string; version: string; includedChars: number };
export type ContextBridgeManifest = { version: 1; entries: { id: string; kind: 'material' | 'history' | 'reference' | 'turn' | 'stable'; status: 'added' | 'retained' | 'updated' | 'reloaded'; sourceId?: string; version: string; textHash?: string; includedChars: number; path?: string; fileName?: string; fileStatus?: string; added?: number | null; deleted?: number | null; binary?: boolean; fixedVersion?: { courseId: string; checkpointCommitOid: string; path: string; blobOid?: string }; locator?: CitationLocator; range?: { startChar: number; endChar: number; complete: boolean; totalBytes: number }; reason: string }[]; projectedChars: number; projectedBytes: number };
export type RequestContext = { focus?: FocusSnapshot; contextBridgeManifest?: ContextBridgeManifest; text: string; manifest: ContextManifestItem[]; historyManifest?: ContextManifestItem[]; assistantReferenceManifest?: ContextManifestItem[]; exerciseReferenceManifest?: ContextManifestItem[]; warnings: string[]; truncated: boolean };
export type ModelSettings = { maxOutputTokens?: number; contextWindow?: number | null; protocol?: string; requestUrl?: string; usageGate?: string; configured: boolean; provider: string; baseUrl: string; model: string; credentialStored: boolean; harness?: string; connected?: boolean };
export type Bootstrap = { courses: Course[]; lastSectionId?: string; lastSessionId?: string; lastCourseId?: string; model: ModelSettings; tasks: Task[]; notices: Notice[]; dataPath?: string; nodes?: LibraryNode[]; treeWarnings?: string[]; recovery?: { status: string; content: string; message?: string } };
export class StationError extends Error {
  code: string;
  details?: unknown;
  constructor(message: string, code = 'UNKNOWN', details?: unknown) { super(message); this.code = code; this.details = details; }
}

export type UiDraft = { key: string; data: { body?: string; question?: string; importTitle?: string; importBody?: string; requestId?: string }; version: string; updatedAt: string };

export type LibraryNodeKind = 'course' | 'folder' | 'section' | 'knowledge' | 'exploration' | 'exercise';
export type LibraryNode = { id: string; title: string; kind: LibraryNodeKind; parentId?: string | null; order?: number; courseId?: string; path: string; version: string; modifiedAt?: string };

export type LibraryTreeSnapshot = { nodes: LibraryNode[]; warnings: string[] };
export type CourseAsset = { id: string; courseId: string; fileName: string; mime: string; size: number; sha256: string; reference: string };
export type CourseAssetPreview = { asset: CourseAsset; text?: string; dataUrl?: string; truncated: boolean };

export type SessionSummary = { id: string; title: string; modifiedAt?: string; path?: string; version?: string };
export type CourseConversations = { courseId: string; activeSessionId: string | null; sessions: SessionSummary[] };
export type TrashPreview = { id: string; title: string; kind: LibraryNodeKind; nodeCount: number; sessionCount: number; version: string };
export type TrashReceipt = { trashId: string; id: string; title: string; kind: LibraryNodeKind; nodeCount: number; sessionCount: number; createdAt: string; restoredAt: string | null; status: 'trashed' | 'restored' };
export type ExplorationNote = { document: Section; session: LearningSession; userEntryId: string; folderId: string };
