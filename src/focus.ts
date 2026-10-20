import type { FocusInput, FocusSnapshot } from './types';
const record = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);
// Ranges use UTF-16 offsets like CodeMirror; Rust resolves their text from the fixed material.
export function captureFocus(documentId: string, body: string, version: string | undefined, view: FocusInput['view'], root: HTMLElement | null, ranges?: { start: number; end: number }[], assetId?: string, questionId?: string): FocusInput {
 const visible = root ? Array.from(root.querySelectorAll<HTMLElement>('[data-focus-start]')).filter(el => {
  const box = el.getBoundingClientRect(), clip = root.getBoundingClientRect();
  return box.height > 0 && box.bottom > Math.max(0, clip.top) && box.top < Math.min(window.innerHeight, clip.bottom);
 }).map(el => ({ start: Number(el.dataset.focusStart), end: Number(el.dataset.focusEnd) })) : [];
 const questions = root ? Array.from(root.querySelectorAll<HTMLElement>('[data-focus-question]')) : [];
 const question = questions.find(el => el.dataset.focusQuestion === questionId) || (root?.contains(document.activeElement) && document.activeElement?.closest<HTMLElement>('[data-focus-question]')) || questions.find(el => { const box = el.getBoundingClientRect(), clip = root!.getBoundingClientRect(); return box.height > 0 && box.bottom > clip.top && box.top < clip.bottom; });
 let budget = 8000;
 const bounded = (view === 'read' || view === 'edit' ? ranges || visible : []).slice(0, 16).flatMap(range => {
  if (!Number.isSafeInteger(range.start) || !Number.isSafeInteger(range.end) || range.start < 0 || range.end < range.start || range.end > body.length || budget <= 0) return [];
  let end = Math.min(range.end, range.start + budget);
  if (end < body.length && /[\uDC00-\uDFFF]/u.test(body[end])) end--;
  budget -= end - range.start;
  return end > range.start ? [{ start: range.start, end }] : [];
 });
 return { documentId, expectedBody: body, ...(version ? { expectedVersion: version } : {}), view, ranges: bounded, ...(question?.dataset.focusQuestion ? { questionId: question.dataset.focusQuestion } : {}), ...(assetId ? { assetId } : {}) };
}
export function readFocus(value: unknown): FocusSnapshot | undefined {
 if (!record(value) || !['id','courseId','documentId','sourceDocumentId','path','type','commitHash','materialTreeOid','documentVersion','view'].every(k => typeof value[k] === 'string') || !/^[a-f0-9]{64}$/u.test(String(value.id)) || !/^[a-f0-9]{40}$/u.test(String(value.commitHash)) || !/^[a-f0-9]{40}$/u.test(String(value.materialTreeOid)) || !['markdown','text','exercise','image','pdf'].includes(String(value.type)) || !['read','edit','exercise','asset'].includes(String(value.view)) || !Array.isArray(value.paragraphs) || value.paragraphs.length > 16) return;
 if (value.paragraphs.some(p => !record(p) || typeof p.text !== 'string' || !Number.isSafeInteger(p.start) || !Number.isSafeInteger(p.end) || Number(p.start) < 0 || Number(p.end) < Number(p.start))) return;
 if (!Array.isArray(value.excludedFiles) || value.excludedFiles.some(f => !record(f) || !['assetId','fileName','reason'].every(k => typeof f[k] === 'string')) || value.paragraphs.reduce((n, p) => n + String(p.text).length, 0) > 16000) return;
 if (value.sessionId !== undefined && (typeof value.sessionId !== 'string' || !value.sessionId)) return;
 if (value.binary !== undefined && typeof value.binary !== 'boolean') return;
 if (value.asset !== undefined && (!record(value.asset) || typeof value.asset.id !== 'string' || typeof value.asset.mime !== 'string' || !Number.isSafeInteger(value.asset.size) || Number(value.asset.size) < 0)) return;
 if (value.exercise !== undefined && (!record(value.exercise) || typeof value.exercise.questionId !== 'string' || !Number.isSafeInteger(value.exercise.number) || Number(value.exercise.number) < 1 || typeof value.exercise.answered !== 'boolean' || typeof value.exercise.graded !== 'boolean')) return;
 return structuredClone(value) as FocusSnapshot;
}
