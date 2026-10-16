export type Source = { title: string; path?: string; excerpt?: string };
export type SectionSummary = { id: string; title: string };
export type Course = { id: string; title: string; sections: SectionSummary[] };
export type Section = { id: string; courseId: string; title: string; body: string; version: string; sourcePath: string; updatedAt?: string };
export type Message = { id: string; role: 'user' | 'assistant'; content: string; createdAt?: string; sources?: Source[] };
export type Exercise = { id: string; entryId?: string; prompt: string; answer?: string; feedback?: string; correct?: boolean };
export type Mistake = { id: string; question: string; reason: string; sectionId: string; createdAt?: string };
export type ReviewCard = { id: string; sessionId?: string; front: string; back: string; sectionId: string; source?: string; dueAt?: string };
export type LearningSession = { id: string; sectionId: string; messages: Message[]; sources?: Source[]; exercises: Exercise[]; mistakes: Mistake[]; cards: ReviewCard[] };
export type Task = { id: string; title: string; dueAt?: string; completed: boolean; reminderEnabled?: boolean };
export type Notice = { id: string; title: string; body?: string; read: boolean };
export type ModelSettings = { configured: boolean; provider: string; baseUrl: string; model: string; credentialStored: boolean; connected?: boolean };
export type Bootstrap = { courses: Course[]; lastSectionId?: string; model: ModelSettings; tasks: Task[]; notices: Notice[]; dataPath?: string; recovery?: { status: string; content: string; message?: string } };
export class StationError extends Error {
  code: string;
  details?: unknown;
  constructor(message: string, code = 'UNKNOWN', details?: unknown) { super(message); this.code = code; this.details = details; }
}

export type UiDraft = { key: string; data: { body?: string; question?: string; importTitle?: string; importBody?: string }; version: string; updatedAt: string };
