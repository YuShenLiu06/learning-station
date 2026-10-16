export type Source = { title: string; path?: string; excerpt?: string };
export type SectionSummary = { id: string; title: string };
export type Course = { id: string; title: string; sections: SectionSummary[] };
export type Section = { id: string; courseId: string; title: string; body: string; version: string; sourcePath: string; updatedAt?: string; createdAt?: string | null; timestampSource?: string; kind?: LibraryNodeKind; parentId?: string | null; originSessionId?: string; exerciseEntryId?: string; importWarning?: string; links?: string[]; draftReady?: boolean; draftError?: string };
export type Message = { id: string; clientMessageId?: string; role: 'user' | 'assistant' | 'note'; needsConfirmation?: boolean; content: string; createdAt?: string; sources?: Source[]; context?: RequestContext };
export type ToolActivity = { id: string; toolCallId: string; runId: string; sessionId: string; name: string; status: 'running' | 'succeeded' | 'failed'; title: string; summary: string; startedAt: string; updatedAt: string; persisted: boolean; object?: { kind: 'todo' | 'learning-entry' | 'review-card'; id: string; sessionId: string; sectionId: string }; errorCode?: string };
export type Exercise = { id: string; entryId?: string; prompt: string; answer?: string; feedback?: string; correct?: boolean; format?: 'exact' | 'self'; reference?: string; explanation?: string; attemptId?: string; cause?: string; causeUpdatedAt?: string };
export type Mistake = { id: string; question: string; reason: string; sectionId: string; createdAt?: string };
export type ReviewCard = { id: string; sessionId?: string; front: string; back: string; sectionId: string; source?: string; dueAt?: string; courseId?: string; reviewState?: { status: 'new' | 'again' | 'understood'; reviewedAt: string | null } };
export type LearningSession = { id: string; sectionId: string; messages: Message[]; toolCalls?: ToolActivity[]; sources?: Source[]; exercises: Exercise[]; mistakes: Mistake[]; cards: ReviewCard[] };
export type Task = { id: string; title: string; dueAt?: string; completed: boolean; reminderEnabled?: boolean };
export type Notice = { id: string; title: string; body?: string; read: boolean };
export type UiPreferences = { fontSize: number; assistantWidth?: number; sidebarCollapsed?: boolean; expandedNodeIds?: string[] };
export type RequestContext = { text: string; manifest: { id: string; path: string; version: string; includedChars: number }[]; historyManifest?: { id: string; path: string; version: string; includedChars: number }[]; warnings: string[]; truncated: boolean };
export type ModelSettings = { protocol?: string; requestUrl?: string; usageGate?: string; configured: boolean; provider: string; baseUrl: string; model: string; credentialStored: boolean; harness?: string; connected?: boolean };
export type Bootstrap = { courses: Course[]; lastSectionId?: string; model: ModelSettings; tasks: Task[]; notices: Notice[]; dataPath?: string; nodes?: LibraryNode[]; treeWarnings?: string[]; recovery?: { status: string; content: string; message?: string } };
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
export type TrashPreview = { id: string; title: string; kind: LibraryNodeKind; nodeCount: number; sessionCount: number; version: string };
export type TrashReceipt = { trashId: string; id: string; title: string; kind: LibraryNodeKind; nodeCount: number; sessionCount: number; createdAt: string; restoredAt: string | null; status: 'trashed' | 'restored' };
export type ExplorationNote = { document: Section; session: LearningSession; userEntryId: string; folderId: string };
