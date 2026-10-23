import { cloneCitation, readLocator } from './citation.ts';
import type { AssistantReference, ContextInjection, ExerciseReference, RequestedContext, FocusSnapshot } from './types';
import { readFocus } from './focus.ts';
import { exerciseReferenceKey } from './exerciseReferences.ts';
import { readCitationContext } from './citationContext.ts';
const validId = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(value);
const bytes = (value: string) => new TextEncoder().encode(value).length;
export function freezeRequestedContext(documentId: string, selection: string, injections: ContextInjection[] = [], assistantReferences: AssistantReference[] = [], exerciseReferences: ExerciseReference[] = [], focus?: FocusSnapshot): RequestedContext {
 return { ...(focus ? { focus: structuredClone(focus) } : {}), documentId: focus?.documentId ?? documentId, selection, injections: injections.map(cloneCitation), ...(assistantReferences.length ? { assistantReferences: assistantReferences.map(cloneCitation) } : {}), ...(exerciseReferences.length ? { exerciseReferences: exerciseReferences.map(cloneCitation) } : {}) };
}
// This is the persisted request intent, never a substitute for a Run's actual manifest.
function parseRequestedContext(value: unknown, entrySelection = ''): RequestedContext | undefined {
 if (!value || typeof value !== 'object' || Array.isArray(value)) return;
 const data = value as Record<string, unknown>;
 if (typeof data.documentId !== 'string' || data.documentId && !validId(data.documentId) || typeof data.selection !== 'string' || bytes(data.selection) > 30000 || data.selection && data.selection !== entrySelection || !Array.isArray(data.injections) || data.injections.length > 8) return;
 const injections: ContextInjection[] = [], seen = new Set<string>();
 for (const raw of data.injections) {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return;
  const item = raw as Record<string, unknown>;
  if (!validId(item.documentId) || seen.has(item.documentId) || typeof item.expectedVersion !== 'string' || !item.expectedVersion || item.expectedVersion.length > 256 || item.selection !== undefined && (typeof item.selection !== 'string' || Array.from(item.selection).length > 8000)) return;
  seen.add(item.documentId);
  injections.push({ documentId: item.documentId, expectedVersion: item.expectedVersion, ...(item.selection === undefined ? {} : { selection: item.selection as string }), ...(item.locator === undefined ? {} : { locator: readLocator(item.locator, item.selection as string) }) });
 }
 const assistantReferences: AssistantReference[] = [], seenAnswers = new Set<string>();
 if (data.assistantReferences !== undefined) {
  if (!Array.isArray(data.assistantReferences) || data.assistantReferences.length > 4 || data.assistantReferences.length + injections.length > 8) return;
  for (const raw of data.assistantReferences) {
   if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return;
   const item = raw as Record<string, unknown>;
   if (!validId(item.sessionId) || !validId(item.entryId) || seenAnswers.has(item.entryId) || item.runId !== undefined && !validId(item.runId) || typeof item.textHash !== 'string' || !/^[a-f0-9]{64}$/u.test(item.textHash) || typeof item.selection !== 'string' || !item.selection || Array.from(item.selection).length > 8000 || bytes(item.selection) > 30000) return;
   seenAnswers.add(item.entryId);
   assistantReferences.push({ sessionId: item.sessionId, entryId: item.entryId, ...(item.runId === undefined ? {} : { runId: item.runId as string }), textHash: item.textHash, selection: item.selection, ...(item.locator === undefined ? {} : { locator: readLocator(item.locator, item.selection) }) });
  }
 }
 const exerciseReferences: ExerciseReference[] = [], seenExercise = new Set<string>();
 if (data.exerciseReferences !== undefined) {
  if (!Array.isArray(data.exerciseReferences) || data.exerciseReferences.length + injections.length + assistantReferences.length > 8) return;
  for (const raw of data.exerciseReferences) {
   if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return;
   const item = raw as Record<string, unknown>;
   if (!validId(item.sessionId) || !validId(item.documentId) || !validId(item.exerciseEntryId) || !validId(item.questionId) || !validId(item.entryId) || !['question','attempt','feedback','reference'].includes(String(item.part)) || typeof item.textHash !== 'string' || !/^[a-f0-9]{64}$/u.test(item.textHash) || typeof item.selection !== 'string' || !item.selection || Array.from(item.selection).length > 8000 || bytes(item.selection) > 30000) return;
   const reference: ExerciseReference = { sessionId: item.sessionId, documentId: item.documentId, exerciseEntryId: item.exerciseEntryId, questionId: item.questionId, part: item.part as ExerciseReference['part'], entryId: item.entryId, textHash: item.textHash, selection: item.selection, ...(item.locator === undefined ? {} : { locator: readLocator(item.locator, item.selection) }) };
   const key = exerciseReferenceKey(reference); if (seenExercise.has(key)) return; seenExercise.add(key); exerciseReferences.push(reference);
  }
 }
 const focus = readFocus(data.focus); if (data.focus != null && (!focus || focus.documentId !== data.documentId)) return;
 return { ...(focus ? { focus } : {}), documentId: data.documentId, selection: data.selection, injections, ...(assistantReferences.length ? { assistantReferences } : {}), ...(exerciseReferences.length ? { exerciseReferences } : {}) };
}

export function readRequestedContext(value: unknown, entrySelection = '', auxiliary?: unknown): RequestedContext | undefined {
 try { const parsed = parseRequestedContext(value, entrySelection); if (!parsed || auxiliary === undefined) return parsed; const citationContext = readCitationContext(auxiliary); return { ...parsed, ...(citationContext ? { citationContext } : { citationContextError: true }) }; } catch { return; }
}
