import type { FocusInput, FocusSnapshot } from './types';
const record = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v);
// “聚焦”＝用户当前正在看的文章：只组装身份、视图与版本元数据。可见范围捕获已随 Focus/Changes 分层移除，ranges 恒为空数组（后端保留字段但忽略）。
export function captureFocus(documentId: string, body: string, version: string | undefined, view: FocusInput['view'], assetId?: string, questionId?: string): FocusInput {
  return { documentId, expectedBody: body, ...(version ? { expectedVersion: version } : {}), view, ranges: [], ...(questionId ? { questionId } : {}), ...(assetId ? { assetId } : {}) };
}
export function readFocus(value: unknown): FocusSnapshot | undefined {
 if (!record(value) || !['id','courseId','documentId','sourceDocumentId','path','type','commitHash','materialTreeOid','documentVersion','view'].every(k => typeof value[k] === 'string') || !/^[a-f0-9]{64}$/u.test(String(value.id)) || !(String(value.courseId) === '' ? String(value.commitHash) === '' && String(value.materialTreeOid) === '' && /^[a-f0-9]{64}$/u.test(String(value.documentVersion)) : /^[a-f0-9]{40}$/u.test(String(value.commitHash)) && /^[a-f0-9]{40}$/u.test(String(value.materialTreeOid))) || !['markdown','text','exercise','image','pdf'].includes(String(value.type)) || !['read','edit','exercise','asset'].includes(String(value.view)) || !Array.isArray(value.paragraphs) || value.paragraphs.length > 16) return;
 if (value.paragraphs.some(p => !record(p) || typeof p.text !== 'string' || !Number.isSafeInteger(p.start) || !Number.isSafeInteger(p.end) || Number(p.start) < 0 || Number(p.end) < Number(p.start))) return;
 if (!Array.isArray(value.excludedFiles) || value.excludedFiles.some(f => !record(f) || !['assetId','fileName','reason'].every(k => typeof f[k] === 'string')) || value.paragraphs.reduce((n, p) => n + String(p.text).length, 0) > 16000) return;
 if (value.sessionId !== undefined && (typeof value.sessionId !== 'string' || !value.sessionId)) return;
 if (value.repositoryVersion !== undefined && value.repositoryVersion !== 2) return;
 if (value.binary !== undefined && typeof value.binary !== 'boolean') return;
 if (value.asset !== undefined && (!record(value.asset) || typeof value.asset.id !== 'string' || typeof value.asset.mime !== 'string' || !Number.isSafeInteger(value.asset.size) || Number(value.asset.size) < 0)) return;
 if (value.exercise !== undefined && (!record(value.exercise) || typeof value.exercise.questionId !== 'string' || !Number.isSafeInteger(value.exercise.number) || Number(value.exercise.number) < 1 || typeof value.exercise.answered !== 'boolean' || typeof value.exercise.graded !== 'boolean')) return;
 return structuredClone(value) as FocusSnapshot;
}
