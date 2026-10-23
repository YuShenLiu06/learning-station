import type { CitationContext, CitationLocator, CitationSupplement, ContextManifestItem } from './types';
const record = (value: unknown): value is Record<string, unknown> => !!value && typeof value === 'object' && !Array.isArray(value);
const integer = (value: unknown): value is number => Number.isSafeInteger(value) && Number(value) >= 0;
const range = (value: unknown): value is CitationLocator => record(value) && integer(value.startChar) && integer(value.endChar) && value.endChar >= value.startChar;
const bytes = (value: string) => new TextEncoder().encode(value).length;
function source(value: unknown): ContextManifestItem | undefined {
 if (!record(value) || typeof value.id !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/u.test(value.id) || typeof value.path !== 'string' || value.path.length > 1024 || typeof value.version !== 'string' || !value.version || value.version.length > 256 || !integer(value.includedChars)) return;
 return { id: value.id, path: value.path, version: value.version, includedChars: value.includedChars };
}
// Saved auxiliary snapshots belong to the original user, never the latest Session.
export function readCitationContext(value: unknown): CitationContext | undefined {
 if (!record(value) || value.version !== 1 || !Array.isArray(value.references) || value.references.length > 8) return;
 const references: CitationContext['references'] = [], seen = new Set<string>();
 for (const raw of value.references) {
  if (!record(raw)) return;
  const origin = source(raw.source);
  if (origin && seen.has(origin.id)) return;
  if (origin) seen.add(origin.id);
  if (!origin || typeof raw.quotedText !== 'string' || !raw.quotedText || origin.includedChars !== Array.from(raw.quotedText).length || raw.locator != null && (!range(raw.locator) || Number(raw.locator.endChar) - Number(raw.locator.startChar) !== origin.includedChars) || !Array.isArray(raw.supplementalContext) || raw.supplementalContext.length > 32) return;
  const supplementalContext: CitationSupplement[] = [];
  let chars = origin.includedChars, size = bytes(raw.quotedText);
  for (const item of raw.supplementalContext) {
   if (!record(item)) return;
   const provenance = source(item.source);
   if (!provenance || !range(item.range) || !range(item.requestedRange) || !integer(item.totalChars) || !integer(item.totalBytes) || typeof item.text !== 'string' || typeof item.truncated !== 'boolean') return;
   const included = item.range as { startChar: number; endChar: number }, requested = item.requestedRange as { startChar: number; endChar: number };
   if (requested.startChar > included.startChar || included.endChar > requested.endChar || requested.endChar > item.totalChars || Array.from(item.text).length !== included.endChar - included.startChar || provenance.includedChars !== included.endChar - included.startChar || item.truncated !== (requested.startChar !== included.startChar || requested.endChar !== included.endChar)) return;
   chars += provenance.includedChars; size += bytes(item.text);
   supplementalContext.push({ source: provenance, range: { ...included }, requestedRange: { ...requested }, totalChars: item.totalChars, totalBytes: item.totalBytes, text: item.text, truncated: item.truncated });
  }
  if (chars > 8000 || size > 30000) return;
  let exerciseState: NonNullable<CitationContext['references'][number]['exerciseState']> | undefined;
  if (raw.exerciseState != null) {
   const state = raw.exerciseState;
   if (!record(state) || typeof state.questionId !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/u.test(state.questionId) || !['unanswered', 'submitted', 'graded'].includes(String(state.status)) || state.attemptId != null && (typeof state.attemptId !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/u.test(state.attemptId)) || state.feedbackEntryId != null && (typeof state.feedbackEntryId !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/u.test(state.feedbackEntryId)) || (state.status === 'unanswered') !== (state.attemptId == null) || state.status === 'graded' && state.feedbackEntryId == null) return;
   exerciseState = { questionId: state.questionId, status: state.status as 'unanswered'|'submitted'|'graded', attemptId: state.attemptId == null ? null : String(state.attemptId), feedbackEntryId: state.feedbackEntryId == null ? null : String(state.feedbackEntryId) };
  }
  references.push({ source: origin, quotedText: raw.quotedText, locator: raw.locator == null ? null : { startChar: Number(raw.locator.startChar), endChar: Number(raw.locator.endChar) }, supplementalContext, ...(exerciseState ? { exerciseState } : {}) });
 }
 return { version: 1, references };
}
